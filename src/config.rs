use crate::gitlab::GitlabAPI;
use crate::{AppResult, Error};
use documented::DocumentedFields;
use serde::Deserialize;
use serde_json::from_str;
use std::fmt::{Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use url::{ParseError, Url};

#[derive(Deserialize, Clone, Debug, DocumentedFields)]
#[allow(dead_code)]
pub struct AppConfig {
    /// Gitlab Personal Access Token for the target user
    pub gitlab_token: SecretString,
    /// Base URL of the Gitlab instance
    pub gitlab_host: Url,
    /// Path to the todotxt file to sync (default = $HOME/.todo/todo.txt)
    #[serde(default = "AppConfig::default_todo_file")]
    pub todo_file: PathBuf,
    /// Context tag to add to synced items. Can be null for none.
    /// If not none, items in the todotxt file without this tag will be ignored
    #[serde(default = "AppConfig::default_context_tag")]
    pub context_tag: Option<String>,
    /// Disable escaping meta tags in Gitlab-originatig text (i.e. key:value will be synced as key\:value)
    #[serde(default)]
    pub no_escape_meta: bool,
    /// Set this to your Gitlab username so item's authors can be specified if they're not you
    /// (TODO: feature not implemented yet)
    #[serde(default)]
    pub username: Option<String>,
    /// Specifies what to do with items marked as done, see [`DonePolicy`] variants
    #[serde(default)]
    pub done_todo_policy: DonePolicy,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq, DocumentedFields)]
#[serde(rename_all = "lowercase")]
pub enum DonePolicy {
    /// Mark todos as done in the output if they were present in the file previously, otherwise skip
    #[default]
    Mark,
    /// Alwauys add done todos to the output
    Add,
    /// Never add done todos to the output. This includes removing preexising todos that are now done
    Ignore,
}

impl AppConfig {
    pub async fn read_from(path: impl AsRef<Path>) -> AppResult<Self> {
        let path = path.as_ref();
        let mut text = String::new();
        File::open(path)
            .await
            .expect(
                format!(
                    "Couldn't open config file {} for reading",
                    path.to_str().unwrap()
                )
                .as_str(),
            )
            .read_to_string(&mut text)
            .await
            .map_err(|e| Error::new(e).context("Couldn't read config file"))?;
        let mut config: AppConfig = from_str(text.as_str()).map_err(|e| Error::new(e))?;

        if let Ok(rel) = config.todo_file.strip_prefix("~") {
            let home = dirs::home_dir().ok_or(Error::msg("Couldn't determine home directory"))?;
            config.todo_file = home.join(rel);
        }

        Ok(config)
    }

    pub fn get_api(&self) -> Result<GitlabAPI, ParseError> {
        GitlabAPI::new(self.gitlab_host.clone(), self.gitlab_token.clone())
    }

    fn default_context_tag() -> Option<String> {
        Some("gitlab".into())
    }

    fn default_todo_file() -> PathBuf {
        dirs::home_dir()
            .expect("Could not determine home dir")
            .join(".todo/todo.txt")
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
            username: None,
            done_todo_policy: Default::default(),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct SecretString(pub String);

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
