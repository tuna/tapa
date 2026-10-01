//! Command and environment values with explicit relocatable local paths.

use camino::{Utf8Path, Utf8PathBuf};

/// A tool value whose local paths can be relocated without changing literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolValue(Vec<Part>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Path(Utf8PathBuf),
}

impl ToolValue {
    pub fn path(path: impl Into<Utf8PathBuf>) -> Self {
        Self(vec![Part::Path(path.into())])
    }

    pub fn prefixed_path(prefix: impl Into<String>, path: impl Into<Utf8PathBuf>) -> Self {
        Self(vec![Part::Literal(prefix.into()), Part::Path(path.into())])
    }

    /// Join values verbatim, for tool formats such as Vitis's CFLAGS string.
    pub fn join(values: impl IntoIterator<Item = Self>, separator: &str) -> Self {
        let mut parts = Vec::new();
        for (index, value) in values.into_iter().enumerate() {
            if index != 0 {
                parts.push(Part::Literal(separator.to_string()));
            }
            parts.extend(value.0);
        }
        Self(parts)
    }

    pub(crate) fn render(&self, resolve: impl Fn(&Utf8Path) -> String) -> String {
        self.0
            .iter()
            .map(|part| match part {
                Part::Literal(value) => value.clone(),
                Part::Path(path) => resolve(path),
            })
            .collect()
    }
}

impl From<String> for ToolValue {
    fn from(value: String) -> Self {
        Self(vec![Part::Literal(value)])
    }
}

impl From<&str> for ToolValue {
    fn from(value: &str) -> Self {
        value.to_string().into()
    }
}

impl std::fmt::Display for ToolValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for part in &self.0 {
            match part {
                Part::Literal(value) => f.write_str(value)?,
                Part::Path(path) => f.write_str(path.as_str())?,
            }
        }
        Ok(())
    }
}
