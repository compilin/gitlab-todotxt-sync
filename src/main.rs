use crate::gitlab::GitlabTodo;
use crate::todo::Todo;
use std::collections::HashMap;

use config::{AppConfig, DonePolicy};
use log::*;
use std::error::Error as StdError;
use std::io::SeekFrom;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{stdout, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};

pub(crate) use anyhow::Error;

mod config;
mod gitlab;
mod todo;

type AppResult<T> = Result<T, Error>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    env_logger::init();
    let config = dirs::config_dir()
        .expect("Could not determine config dir")
        .join("gitlab-todotxt-sync/config.json");
    let config = AppConfig::read_from(&config).await?;

    let gltodos = get_gitlab_todos(&config).await?;

    let todos = gltodos
        .into_iter()
        .map(|gl| -> AppResult<(usize, Todo)> {
            let id = gl.id;
            gl.into_todo(&config).map(|t| (id, t))
        })
        .collect::<AppResult<HashMap<_, _>>>()?;

    let mut tf = File::options()
        .read(true)
        .create(true)
        .write(true)
        .open(&config.todo_file)
        .await?;

    let existing = read_existing(&config, &mut tf).await?;
    let splitf = |t: &Todo| {
        config
            .context_tag
            .as_ref()
            .map(|ctx| t.has_context(ctx))
            .unwrap_or(true)
    };
    let (mut existing, other): (Vec<_>, _) = existing.into_iter().partition(splitf);
    update_todos(
        &mut existing,
        todos,
        config.done_todo_policy == DonePolicy::Add,
    );
    let todos = [other, existing].concat();

    let mut buf: Vec<u8> = Vec::new();
    Todo::write_file(&mut buf, todos.iter()).await?;

    info!(
        "Writing {} todos to file ({} bytes):",
        todos.len(),
        buf.len()
    );
    stdout().write_all(&buf).await?;

    tf.set_len(0).await?; // Truncate file
    tf.seek(SeekFrom::Start(0)).await?;
    tf.write_all(&buf).await?;
    tf.flush().await?;

    Ok(())
}

async fn get_gitlab_todos(config: &AppConfig) -> Result<Vec<GitlabTodo>, Box<dyn StdError>> {
    let api = config.get_api()?;

    let gltodos: Vec<GitlabTodo> = if let Ok(json) = std::env::var("GITLAB_TODOS_JSON") {
        info!("Loading from file {json}");
        let mut todos: Vec<GitlabTodo> = from_file(json).await?;
        if let DonePolicy::Ignore = config.done_todo_policy {
            todos.retain(|t| !t.is_done());
        }
        todos
    } else if let DonePolicy::Ignore = config.done_todo_policy {
        api.get_pending_todos().await?
    } else {
        api.get_all_todos().await?
    };
    Ok(gltodos)
}

async fn read_existing(config: &AppConfig, tf: &mut File) -> Result<Vec<Todo>, Box<dyn StdError>> {
    let existing = Todo::read_file(tf).await?;
    info!(
        "Read {} existing todos from {}",
        existing.len(),
        config.todo_file.to_str().unwrap()
    );
    Ok(existing)
}

fn update_todos(
    existing: &mut Vec<Todo>,
    mut todos: HashMap<usize, Todo>,
    add_done: bool,
) -> (usize, usize, usize) {
    fn get_id(t: &Todo) -> Option<usize> {
        match t
            .get_data("id")
            .ok_or("Todo is missing an id data tag".to_string())
            .and_then(|id| {
                id.parse::<usize>()
                    .map_err(|_| format!("Couldn't parse id as usize: {id}"))
            }) {
            Ok(id) => Some(id),
            Err(e) => {
                warn!("{}", e);
                None
            }
        }
    }
    let mut upd = 0;
    let mut del = 0;
    existing.retain_mut(|extd| {
        if let Some(id) = get_id(extd) {
            if let Some(td) = todos.remove(&id) {
                if extd != &td {
                    upd += 1;
                    *extd = td;
                }
            } else {
                del += 1;
                return false;
            }
        }
        true
    });
    let new = todos.len();
    if add_done {
        existing.extend(todos.into_values());
    } else {
        existing.extend(todos.into_values().filter(|t| !t.done));
    }
    (new, upd, del)
}

async fn from_file<T: serde::de::DeserializeOwned>(path: impl AsRef<Path>) -> AppResult<T> {
    from_async_reader(File::open(path).await.map_err(Error::from)?).await
}

async fn from_async_reader<R, T>(rdr: R) -> AppResult<T>
where
    R: tokio::io::AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut buf = vec![];
    BufReader::new(rdr)
        .read_to_end(&mut buf)
        .await
        .map_err(Error::from)?;
    serde_json::from_slice(&buf).map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use crate::todo::{DescriptionPart, Todo};
    use crate::update_todos;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_done_policy() {
        let t1 = Todo::new(false, None, None, None, "Test 1 id:1 +test".to_string()).unwrap();
        let t2 = Todo::new(false, None, None, None, "Test 2 id:2 +test".to_string()).unwrap();
        let t3 = Todo::new(false, None, None, None, "Test 3 id:3 +test".to_string()).unwrap();
        let mut t1d = t1.clone();
        t1d.done = true;
        let mut t2d = t2.clone();
        t2d.done = true;
        let mut t3d = t3.clone();
        t3d.done = true;

        fn test(mut existing: Vec<Todo>, todos: Vec<Todo>, result: &[Todo], add_done: bool) {
            update_todos(&mut existing, map_of(todos), add_done);
            let existing: HashSet<(&str, bool)> =
                HashSet::from_iter(existing.iter().map(|t| (t.get_data("id").unwrap(), t.done)));
            let result =
                HashSet::from_iter(result.iter().map(|t| (t.get_data("id").unwrap(), t.done)));
            assert_eq!(
                existing, result,
                "Testing update_todos with add_done = {:?}",
                add_done
            );
        }

        test(
            vec![],
            vec![t1d.clone(), t2d.clone()],
            &[t1d.clone(), t2d.clone()],
            true,
        );

        test(
            vec![t1.clone()],
            vec![t1d.clone(), t2.clone(), t3d.clone()],
            &[t1d.clone(), t2.clone()],
            false,
        );
    }

    #[test]
    fn test_tag_escape() {
        const PRJ: &str = "testprj";
        const CTX: &str = "testctx";
        const DATAK: &str = "test";
        const DATAV: &str = "data";
        let todo = Todo::new(false, None, None, None, "Test".into()).unwrap()
            + DescriptionPart::Project(PRJ)
            + DescriptionPart::Context(CTX)
            + DescriptionPart::Data(DATAK, DATAV);
        let escaped = Todo::new(
            false,
            None,
            None,
            None,
            Todo::escape_description(&todo.description).to_string(),
        )
        .unwrap();
        assert_eq!(
            escaped.find_meta().collect::<Vec<_>>(),
            vec![],
            "Escaped description shouldn't return any meta"
        );
        for s in [PRJ, CTX, DATAK, DATAV] {
            assert!(
                escaped.description.find(s).is_some(),
                "Escaped description should still contain '{}'",
                s
            );
        }
    }

    fn map_of(tds: impl IntoIterator<Item = Todo>) -> HashMap<usize, Todo> {
        HashMap::from_iter(
            tds.into_iter()
                .map(|t| (t.get_data("id").unwrap().parse().unwrap(), t)),
        )
    }
}
