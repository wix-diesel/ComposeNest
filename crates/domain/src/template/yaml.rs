use yaml_rust2::{
    parser::{Event, Parser},
    scanner::{Marker, TScalarStyle},
};

/// A source position in a template document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// One-based line number.
    pub line: usize,
    /// One-based column number.
    pub column: usize,
}

impl From<Marker> for Position {
    fn from(marker: Marker) -> Self {
        Self {
            line: marker.line(),
            column: marker.col() + 1,
        }
    }
}

/// A restricted YAML value with insertion order and source location retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The value's location in its source document.
    pub position: Position,
    /// The parsed JSON-compatible value.
    pub value: Value,
}

/// Values accepted by the restricted YAML grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A YAML string, including quoted numeric text.
    String(String),
    /// A signed integer.
    Integer(i64),
    /// A boolean.
    Boolean(bool),
    /// An ordered mapping.
    Map(Vec<(String, Node)>),
    /// A sequence.
    Sequence(Vec<Node>),
}

/// A template validation error tied to the source file and item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    /// Template package identifier or display name supplied by the caller.
    pub package: Box<str>,
    /// Version key for a version document, if known.
    pub version: Option<Box<str>>,
    /// Package-relative source filename.
    pub file: Box<str>,
    /// Source position, if available.
    pub position: Option<Position>,
    /// Dot-separated item path.
    pub path: Box<str>,
    /// Reason and suggested correction.
    pub message: Box<str>,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.path, self.message)
    }
}

impl std::error::Error for TemplateError {}

pub(super) struct Context<'a> {
    pub package: &'a str,
    pub version: Option<&'a str>,
    pub file: &'a str,
}

impl Context<'_> {
    pub fn error(
        &self,
        position: Option<Position>,
        path: &str,
        message: impl Into<String>,
    ) -> TemplateError {
        TemplateError {
            package: self.package.into(),
            version: self.version.map(Into::into),
            file: self.file.into(),
            position,
            path: path.into(),
            message: message.into().into_boxed_str(),
        }
    }
}

pub(super) fn parse(bytes: &[u8], context: &Context<'_>) -> Result<Node, TemplateError> {
    if bytes.len() > 256 * 1024 {
        return Err(context.error(None, "$", "file exceeds the 256 KiB limit"));
    }
    let source =
        std::str::from_utf8(bytes).map_err(|_| context.error(None, "$", "file must be UTF-8"))?;
    let mut reader = EventReader {
        parser: Parser::new_from_str(source),
        context,
    };
    reader.read_document()
}

struct EventReader<'a, 'b> {
    parser: Parser<std::str::Chars<'a>>,
    context: &'b Context<'b>,
}

impl EventReader<'_, '_> {
    fn next(&mut self, path: &str) -> Result<(Event, Position), TemplateError> {
        self.parser
            .next_token()
            .map(|(event, marker)| (event, marker.into()))
            .map_err(|error| {
                let message = if error.to_string().contains("unknown anchor") {
                    "YAML aliases are not allowed".to_owned()
                } else {
                    error.to_string()
                };
                self.context
                    .error(Some((*error.marker()).into()), path, message)
            })
    }

    fn read_document(&mut self) -> Result<Node, TemplateError> {
        if !matches!(self.next("$")?.0, Event::StreamStart)
            || !matches!(self.next("$")?.0, Event::DocumentStart)
        {
            return Err(self.context.error(None, "$", "expected one YAML document"));
        }
        let first = self.next("$")?;
        let root = self.read_value(first, "$", 1)?;
        if !matches!(self.next("$")?.0, Event::DocumentEnd)
            || !matches!(self.next("$")?.0, Event::StreamEnd)
        {
            return Err(self
                .context
                .error(None, "$", "only one YAML document is allowed"));
        }
        Ok(root)
    }

    fn read_value(
        &mut self,
        (event, position): (Event, Position),
        path: &str,
        depth: usize,
    ) -> Result<Node, TemplateError> {
        if depth > 16 {
            return Err(self.context.error(
                Some(position),
                path,
                "nesting exceeds the depth limit of 16",
            ));
        }
        let value = match event {
            Event::Scalar(text, style, 0, None) => {
                if text.len() > 16 * 1024 {
                    return Err(self.context.error(
                        Some(position),
                        path,
                        "scalar exceeds the 16 KiB limit",
                    ));
                }
                parse_scalar(text, style).ok_or_else(|| self.context.error(Some(position), path,
                    "null and non-integer numbers are unsupported; quote numeric text to use a string"))?
            }
            Event::MappingStart(0, None) => {
                let mut entries = Vec::new();
                loop {
                    let key_event = self.next(path)?;
                    if matches!(key_event.0, Event::MappingEnd) {
                        break;
                    }
                    let (Event::Scalar(key, style, 0, None), key_position) = key_event else {
                        return Err(self.context.error(
                            Some(key_event.1),
                            path,
                            "mapping keys must be strings without tags or anchors",
                        ));
                    };
                    if !matches!(parse_scalar(key.clone(), style), Some(Value::String(_)))
                        || key == "<<"
                    {
                        return Err(self.context.error(Some(key_position), path, "mapping keys must be strings; quote numeric keys and do not use merge keys"));
                    }
                    let item_path = format!("{path}.{key}");
                    if entries.iter().any(|(existing, _)| existing == &key) {
                        return Err(self.context.error(
                            Some(key_position),
                            &item_path,
                            "duplicate key; remove the repeated entry",
                        ));
                    }
                    let next = self.next(&item_path)?;
                    entries.push((key, self.read_value(next, &item_path, depth + 1)?));
                }
                Value::Map(entries)
            }
            Event::SequenceStart(0, None) => {
                let mut items = Vec::new();
                loop {
                    let next = self.next(path)?;
                    if matches!(next.0, Event::SequenceEnd) {
                        break;
                    }
                    let item_path = format!("{path}[{}]", items.len());
                    items.push(self.read_value(next, &item_path, depth + 1)?);
                }
                Value::Sequence(items)
            }
            Event::Alias(_) => {
                return Err(self.context.error(
                    Some(position),
                    path,
                    "YAML aliases are not allowed",
                ));
            }
            Event::Scalar(_, _, _, _) | Event::SequenceStart(_, _) | Event::MappingStart(_, _) => {
                return Err(self.context.error(
                    Some(position),
                    path,
                    "YAML anchors and tags are not allowed",
                ));
            }
            _ => {
                return Err(self
                    .context
                    .error(Some(position), path, "expected a YAML value"));
            }
        };
        Ok(Node { position, value })
    }
}

fn parse_scalar(text: String, style: TScalarStyle) -> Option<Value> {
    if style != TScalarStyle::Plain {
        return Some(Value::String(text));
    }
    match text.as_str() {
        "" | "null" | "Null" | "NULL" | "~" => None,
        "true" => Some(Value::Boolean(true)),
        "false" => Some(Value::Boolean(false)),
        _ if text.parse::<i64>().is_ok() && is_json_integer(&text) => {
            Some(Value::Integer(text.parse().ok()?))
        }
        _ if looks_numeric(&text) => None,
        _ => Some(Value::String(text)),
    }
}

fn is_json_integer(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && (digits == "0"
            || (!digits.starts_with('0') && digits.bytes().all(|byte| byte.is_ascii_digit())))
}

fn looks_numeric(value: &str) -> bool {
    let first = value.as_bytes().first().copied();
    matches!(first, Some(b'0'..=b'9' | b'-' | b'+'))
        && (value.parse::<f64>().is_ok() || value.starts_with("0x") || value.starts_with("0o"))
        || matches!(value, ".nan" | ".NaN" | ".NAN" | ".inf" | ".Inf" | ".INF")
}
