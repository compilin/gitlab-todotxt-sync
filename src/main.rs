use crate::gitlab::{GitlabAPI, GitlabTodo};
use crate::todo::Todo;
use std::collections::HashMap;

use log::*;
use serde::Deserialize;
use serde_json::from_str;
use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter};
use std::io::SeekFrom;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{stdout, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use url::{ParseError, Url};

mod todo;
mod gitlab;

type AppResult<T> = Result<T, Error>;

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
struct AppConfig {
    gitlab_token: SecretString,
    gitlab_host: Url,
    todo_file: PathBuf,
    context_tag: Option<String>,
    #[serde(default)]
    no_escape_meta: bool,
    #[serde(default)]
    fetch_done: bool,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    done_todo_policy: DonePolicy,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
enum DonePolicy {
    // Mark todos as done in the output if they were present in the file previously, otherwise skip
    #[default]
    Mark,
    // Alwauys add done todos to the output
    Add,
    // Never add done todos to the output. This includes removing preexising todos that are now done
    Ignore,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let config = dirs::config_dir().expect("Could not determine config dir")
        .join("gitlab-todotxt-sync/config.json");
    let config = AppConfig::read_from(&config).await?;

    let gltodos = get_gitlab_todos(&config).await?;

    let todos = gltodos.into_iter()
        .map(|gl| -> AppResult<(usize, Todo)> {
            let id = gl.id;
            gl.into_todo(&config).map(|t| (id, t))
        }).collect::<AppResult<HashMap<_, _>>>()?;

    let mut tf = File::options()
        .read(true)
        .create(true)
        .write(true)
        .open(&config.todo_file).await?;

    let existing = read_existing(&config, &mut tf).await?;
    let splitf = |t: &Todo| config.context_tag.as_ref().map(
        |ctx| t.has_context(ctx)).unwrap_or(true);
    let (mut existing, other): (Vec<_>, _) = existing.into_iter().partition(splitf);
    update_todos(&mut existing, todos, config.done_todo_policy == DonePolicy::Add);
    let todos = [other, existing].concat();

    let mut buf: Vec<u8> = Vec::new();
    Todo::write_file(&mut buf, todos.iter()).await?;

    info!("Writing {} todos to file ({} bytes):", todos.len(), buf.len());
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
    info!("Read {} existing todos from {}", existing.len(), config.todo_file.to_str().unwrap());
    Ok(existing)
}


fn update_todos(existing: &mut Vec<Todo>, mut todos: HashMap<usize, Todo>, add_done: bool) -> (usize, usize, usize) {
    fn get_id(t: &Todo) -> Option<usize> {
        match t.get_data("id").ok_or("Todo is missing an id data tag".to_string())
            .and_then(|id| id.parse::<usize>()
                .map_err(|_| format!("Couldn't parse id as usize: {id}"))) {
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

impl AppConfig {
    async fn read_from(path: impl AsRef<Path>) -> AppResult<Self> {
        let path = path.as_ref();
        let mut text = String::new();
        File::open(path).await
            .expect(format!("Couldn't open config file {} for reading", path.to_str().unwrap()).as_str())
            .read_to_string(&mut text).await
            .map_err(|e| Error::from_msg("Couldn't read config file", e))?;
        let mut config: AppConfig = from_str(text.as_str())
            .map_err(|e| Error::new(e.to_string()))?;

        if let Ok(rel) = config.todo_file.strip_prefix("~") {
            let home = dirs::home_dir()
                .ok_or(Error::new("Couldn't determine home directory"))?;
            config.todo_file = home.join(rel);
        }

        Ok(config)
    }

    fn get_api(&self) -> Result<GitlabAPI, ParseError> {
        GitlabAPI::new(self.gitlab_host.clone(), self.gitlab_token.clone())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            gitlab_token: SecretString("glpat-YOUR-GITLAB-TOKEN".into()),
            gitlab_host: Url::parse("https://git.domain.example").unwrap(),
            todo_file: Default::default(),
            context_tag: None,
            no_escape_meta: false,
            fetch_done: false,
            username: None,
            done_todo_policy: Default::default(),
        }
    }
}

#[derive(Debug)]
pub struct Error {
    msg: String,
    cause: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    pub const DEFAULT_MESSAGE: &'static str = "Internal error";

    pub fn new<S: ToString>(description: S) -> Error {
        Self { msg: description.to_string(), cause: None }
    }

    pub fn from(e: impl StdError + Send + Sync + 'static) -> Self {
        Self::from_msg(Self::DEFAULT_MESSAGE, e)
    }

    pub fn from_msg(msg: impl ToString, err: impl StdError + Send + Sync + 'static) -> Self {
        Self {
            msg: msg.to_string(),
            cause: Some(Box::new(err)),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause.as_ref().map(|c| -> &(dyn StdError + 'static) { c.deref() })
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (self.msg == Self::DEFAULT_MESSAGE, &self.cause) {
            (_, None) => write!(f, "{}", self.msg),
            (false, Some(cause)) => write!(f, "{}: {}", self.msg, cause),
            (true, Some(cause)) => write!(f, "{}", cause),
        }
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(value: std::num::ParseIntError) -> Self {
        Self {
            msg: format!("ParseIntError: {value}"),
            cause: Some(Box::new(value)),
        }
    }
}

#[derive(Clone, Deserialize)]
struct SecretString(String);

impl AsRef<str> for SecretString {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Debug for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretString({})", self)
    }
}

impl Display for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("**REDACTED**")
    }
}

async fn from_file<T: serde::de::DeserializeOwned>(path: impl AsRef<Path>) -> AppResult<T> {
    from_async_reader(
        File::open(path).await.map_err(Error::from)?).await
}

async fn from_async_reader<R, T>(rdr: R) -> AppResult<T>
where
    R: tokio::io::AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut buf = vec![];
    BufReader::new(rdr)
        .read_to_end(&mut buf).await
        .map_err(Error::from)?;
    serde_json::from_slice(&buf)
        .map_err(Error::from)
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
            let existing: HashSet<(&str, bool)> = HashSet::from_iter(existing.iter()
                .map(|t| (t.get_data("id").unwrap(), t.done)));
            let result = HashSet::from_iter(result.iter()
                .map(|t| (t.get_data("id").unwrap(), t.done)));
            assert_eq!(existing, result, "Testing update_todos with add_done = {:?}", add_done);
        }

        test(vec![],
             vec![t1d.clone(), t2d.clone()],
             &[t1d.clone(), t2d.clone()], true);

        test(vec![t1.clone()],
             vec![t1d.clone(), t2.clone(), t3d.clone()],
             &[t1d.clone(), t2.clone()], false);
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
        let escaped = Todo::new(false, None, None, None,
            Todo::escape_description(&todo.description).to_string()).unwrap();
        assert_eq!(escaped.find_meta().count(), 0, "Escaped description shouldn't return any meta");
        for s in [PRJ, CTX, DATAK, DATAV] {
            assert!(escaped.description.find(s).is_some(),
                "Escaped description should still contain '{}'", s);
        }
    }


    fn map_of(tds: impl IntoIterator<Item=Todo>) -> HashMap<usize, Todo> {
        HashMap::from_iter(tds.into_iter()
            .map(|t| (t.get_data("id").unwrap().parse().unwrap(), t)))
    }
}
