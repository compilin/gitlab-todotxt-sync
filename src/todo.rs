use std::borrow::{Borrow, Cow};
use crate::{AppResult, Error};
use regex::{Captures, Regex};
use std::fmt::{Display, Formatter};
use std::ops::{Add, AddAssign};
use std::str::FromStr;
use std::sync::LazyLock;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, PartialEq)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DescriptionPart<'a> {
    Project(&'a str),
    Context(&'a str),
    Data(&'a str, &'a str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub done: bool,
    pub priority: Option<char>,
    pub created: Option<Date>,
    pub completed: Option<Date>,
    pub description: String,
}

#[allow(dead_code)]
impl Todo {
    pub fn new(done: bool, priority: Option<char>, created: Option<Date>,
               completed: Option<Date>, description: String) -> AppResult<Self> {
        if completed.is_some() && created.is_none() {
            return Err(Error::new("Can't have a Todo with a completion date and not a creation date"))
        }
        Ok(Self { done, priority, created, completed, description })
    }

    fn part_reg() -> &'static Regex {
        static PART_REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(
            r"(^|\s)(?<tag>(?<head>@|\+|(?<key>\w+):)(?<body>\S)+)\b").unwrap());
        &*PART_REG
    }

    pub fn find_meta(&self) -> impl Iterator<Item=DescriptionPart> {
        Self::part_reg().captures_iter(&self.description)
            .map(|c| DescriptionPart::parse(c.name("tag").unwrap().as_str())
                .expect("Can't parse DescriptionPart"))
    }

    pub fn get_tag(&self) -> Vec<DescriptionPart> {
        self.find_meta().collect()
    }

    pub fn has_tag(&self, tag: DescriptionPart) -> bool {
        self.find_meta().find(|t| t == &tag).is_some()
    }

    pub fn has_context(&self, ctx: impl AsRef<str>) -> bool {
        self.has_tag(DescriptionPart::Context(ctx.as_ref()))
    }

    pub fn has_project(&self, ctx: impl AsRef<str>) -> bool {
        self.has_tag(DescriptionPart::Project(ctx.as_ref()))
    }

    pub fn get_data(&self, ctx: impl AsRef<str>) -> Option<&str> {
        self.find_meta()
            .filter_map(|t| match t {
                DescriptionPart::Data(k, v) if k == ctx.as_ref() => Some(v),
                _ => None,
            }).next()
    }

    pub fn escape_description(desc: &str) -> Cow<str> {
        Self::part_reg().replace_all(desc.as_ref(), |c: &Captures| {
            let pos = c.name("head").unwrap().end() - 1;
            let m = &c.get(0).unwrap();
            format!("{}\\{}", &desc[m.start()..pos], &desc[pos..m.end()])
        })
    }

    pub async fn read_file(f: impl AsyncRead + Unpin) -> AppResult<Vec<Self>> {
        let mut vec = Vec::new();
        let mut lines = BufReader::new(f).lines();
        while let Some(line) = lines.next_line().await.map_err(Error::from)? {
            if !line.trim().is_empty() {
                vec.push(line.parse()?);
            }
        }
        Ok(vec)
    }

    pub async fn write_file(mut f: impl AsyncWrite + Unpin,
                            todos: impl IntoIterator<Item=impl Borrow<Todo>>) -> AppResult<()> {
        use std::io::Write;
        let mut buf: Vec<u8> = Vec::new();
        for todo in todos {
            writeln!(&mut buf, "{}", todo.borrow()).map_err(Error::from)?;
        }
        f.write_all(&buf).await.map_err(Error::from)
    }
}

impl Display for Todo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.done {
            f.write_str("x ")?;
        }
        if let Some(pri) = self.priority {
            write!(f, "({}) ", pri)?;
        }
        if let Some(comp) = &self.completed {
            write!(f, "{} ", comp)?;
        }
        if let Some(crea) = &self.created {
            write!(f, "{} ", crea)?;
        }
        write!(f, "{}", self.description)?;

        Ok(())
    }
}

impl FromStr for Todo {
    type Err = Error;

    fn from_str(mut s: &str) -> AppResult<Self> {
        let mut done: bool = false;
        let mut priority: Option<char> = None;
        let mut created: Option<Date> = None;
        let mut completed: Option<Date> = None;

        if s.starts_with("x ") {
            done = true;
            s = &s[2..];
        }

        static PRI_REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\(?<pri>([A-Z])\)\s+").unwrap());
        if let Some(pri) = PRI_REG.captures(s) {
            s = &s[pri.get(0).unwrap().len()..];
            priority = Some(pri.name("pri").unwrap().as_str().chars().next().unwrap());
        }

        if let Some((part, rest)) = s.split_once(char::is_whitespace) {
            if let Ok(date) = Date::from_str(part) {
                if let Some((creat, rest)) = rest.split_once(char::is_whitespace)
                    .and_then(|(part, rest)| Date::from_str(part).ok().map(|d| (d, rest))) {
                    if !done {
                        return Err(Error::new("Completion date present on uncompleted todo"))
                    }
                    s = rest;
                    created = Some(creat);
                    completed = Some(date);
                } else {
                    s = rest;
                    created = Some(date);
                }
            }
        }

        Self::new(done, priority, created, completed, s.trim_start().to_string())
    }
}

impl Display for Date {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl FromStr for Date {
    type Err = Error;

    fn from_str(s: &str) -> AppResult<Self> {
        if let Some((year, rest)) = s.split_once('-') {
            if let Some((month, day)) = rest.split_once('-') {
                return Ok(Self {
                    year: year.parse()?,
                    month: month.parse()?,
                    day: day.parse()?,
                })
            }
        }
        Err(Error::new("Invalid date format"))
    }
}

impl<'a> DescriptionPart<'a> {
    fn parse(s: &'a str) -> AppResult<Self> {
        if s.starts_with('@') {
            Ok(DescriptionPart::Context(&s[1..]))
        } else if s.starts_with('+') {
            Ok(DescriptionPart::Project(&s[1..]))
        } else if let Some((k, v)) = s.split_once(':') {
            Ok(DescriptionPart::Data(k, v))
        } else {
            Err(Error::new(format!("Couldn't parse DescriptionPart '{s}'")))
        }
    }
}

impl Display for DescriptionPart<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DescriptionPart::Project(p) => write!(f, "+{p}"),
            DescriptionPart::Context(c) => write!(f, "@{c}"),
            DescriptionPart::Data(k, v) => write!(f, "{k}:{v}"),
        }
    }
}

impl AddAssign<DescriptionPart<'_>> for Todo {
    fn add_assign(&mut self, rhs: DescriptionPart) {
        self.description = if self.description.is_empty() {
            rhs.to_string()
        } else if self.description.ends_with(char::is_whitespace) {
            self.description.clone() + &rhs.to_string()
        } else {
            format!("{} {}", self.description, rhs)
        };
    }
}

impl<T> Add<T> for Todo where Todo: AddAssign<T> {
    type Output = Self;

    fn add(mut self, rhs: T) -> Self::Output {
        self += rhs;
        self
    }
}