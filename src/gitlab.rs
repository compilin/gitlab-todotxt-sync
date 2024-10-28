use std::borrow::Cow;
use crate::todo::{Date, DescriptionPart, Todo};
use crate::{AppConfig, AppResult, Error, SecretString};
use reqwest::{IntoUrl, Method, RequestBuilder};
use serde::de::Error as SerdeError;
use serde::Deserialize;
use url::Url;

const API_BASE: &str = "api/v4/";
const STATE_PENDING: &str = "pending";
const STATE_DONE: &str = "done";

#[derive(Debug, Clone)]
pub struct GitlabAPI {
    client: reqwest::Client,
    base: Url,
    token: SecretString,
}

#[allow(dead_code)]
impl GitlabAPI {
    pub fn new(base: Url, token: impl AsRef<str>) -> Result<Self, url::ParseError> {
        Ok(Self {
            client: reqwest::Client::new(),
            base: base.join(API_BASE)?,
            token: SecretString(token.as_ref().to_owned()),
        })
    }

    fn request(&self, method: Method, u: impl IntoUrl) -> RequestBuilder {
        const AUTH_HEADER: &str = "PRIVATE-TOKEN";
        self.client.request(method, u)
            .header(AUTH_HEADER, self.token.as_ref())
    }

    fn get(&self, u: impl IntoUrl) -> RequestBuilder {
        self.request(Method::GET, u)
    }

    async fn get_todos(&self, pending: bool) -> reqwest::Result<Vec<GitlabTodo>> {
        const TODO_ENDPOINT: &str = "todos/";
        let pending = if pending { STATE_PENDING } else { STATE_DONE };
        let url = self.base.join(TODO_ENDPOINT).unwrap();
        let request = self.get(url.clone())
            .query(&[("state", pending)]);
        print!("GET {url} -> ");
        let response = request
            .send()
            .await?;
        println!("{response:?}");
        response.json()
            .await
    }

    pub async fn get_pending_todos(&self) -> reqwest::Result<Vec<GitlabTodo>> {
        self.get_todos(true).await
    }

    pub async fn get_done_todos(&self) -> reqwest::Result<Vec<GitlabTodo>> {
        self.get_todos(false).await
    }

    pub async fn get_all_todos(&self) -> reqwest::Result<Vec<GitlabTodo>> {
        Ok([
            self.get_todos(true).await?,
            self.get_todos(false).await?
        ].concat())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GitlabTodo {
    pub id: usize,
    pub body: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub action_name: String,
    pub target_type: String,
    #[serde(deserialize_with="get_username", default)]
    pub author: Option<String>,
    #[serde(deserialize_with="get_entity_path", default)]
    pub project: Option<String>,
    #[serde(deserialize_with="get_entity_path", default)]
    pub group: Option<String>,
    pub target_url: Url,
}

macro_rules! get_struct_field {
    ($func:ident($field:ident) -> Option: $ty:ty) => {
        fn $func<'de, D>(de: D) -> Result<Option<$ty>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            get_struct_field!($func($field) -> $ty);
            $func(de).map(|r| Some(r))
        }
    };
    ($func:ident($field:ident) -> $ty:ty) => {
        fn $func<'de, D>(de: D) -> Result<$ty, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            #[derive(Deserialize)]
            pub struct Struct {
                $field: $ty,
            }

            Option::<Struct>::deserialize(de)
                .and_then(|o| o.ok_or(D::Error::missing_field(stringify!($field))))
                .map(|e| e.$field)
                .map_err(|e| D::Error::custom(e.to_string()))
        }
    };
}

get_struct_field!(get_entity_path(path_with_namespace) -> Option: String);
get_struct_field!(get_username(username) -> Option: String);

impl GitlabTodo {
    pub fn into_todo(self, config: &AppConfig) -> Result<Todo, Error> {
        use std::str::FromStr;
        fn parse_date(raw: impl AsRef<str>) -> AppResult<Date> {
            let raw = raw.as_ref();
            raw.split_once('T')
                .ok_or(Error::new(format!("Couldn't parse date from '{raw}'")))
                .and_then(|(d, _)| Date::from_str(d))
        }
        let done = self.is_done();

        let description = if config.no_escape_meta {
            Cow::Borrowed(self.body.as_str())
        } else {
            Todo::escape_description(self.body.as_str())
        };
        let mut result = Todo::new(done,
                                   None,
                                   Some(parse_date(self.created_at)?),
                                   if done { Some(parse_date(self.updated_at)?) } else { None },
                                   format!("[{}:{}] {}", self.target_type, self.action_name, description))?;

        if let Some(proj) = self.project {
            result += DescriptionPart::Project(&proj);
        } else if let Some(group) = self.group {
            result += DescriptionPart::Project(&group);
        }
        result += DescriptionPart::Data("id", &self.id.to_string());
        if let Some(ctx) = &config.context_tag {
            result += DescriptionPart::Context(ctx)
        }

        Ok(result)
    }

    pub fn is_done(&self) -> bool {
        self.state == STATE_DONE
    }
}

