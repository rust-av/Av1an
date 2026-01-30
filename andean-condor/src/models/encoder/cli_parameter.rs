use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CLIParameter {
    String {
        prefix:    String,
        delimiter: String,
        value:     String,
    },
    Number {
        prefix:    String,
        delimiter: String,
        value:     f64,
    },
    Bool {
        prefix: String,
        value:  bool,
    },
}

impl CLIParameter {
    #[inline]
    pub fn new_string(prefix: &str, delimiter: &str, value: &str) -> CLIParameter {
        CLIParameter::String {
            prefix:    prefix.to_owned(),
            delimiter: delimiter.to_owned(),
            value:     value.to_owned(),
        }
    }

    #[inline]
    pub fn new_strings(
        prefix: &str,
        delimiter: &str,
        values: &[(&str, &str)],
    ) -> HashMap<String, CLIParameter> {
        values.iter().fold(
            HashMap::<String, CLIParameter>::new(),
            |mut map, (key, value)| {
                map.insert(
                    key.to_owned().to_owned(),
                    CLIParameter::new_string(prefix, delimiter, value),
                );
                map
            },
        )
    }

    #[inline]
    pub fn new_number(prefix: &str, delimiter: &str, value: f64) -> CLIParameter {
        CLIParameter::Number {
            prefix: prefix.to_owned(),
            delimiter: delimiter.to_owned(),
            value,
        }
    }

    #[inline]
    pub fn new_numbers(
        prefix: &str,
        delimiter: &str,
        values: &[(&str, f64)],
    ) -> HashMap<String, CLIParameter> {
        values.iter().fold(
            HashMap::<String, CLIParameter>::new(),
            |mut map, (key, value)| {
                map.insert(
                    key.to_owned().to_owned(),
                    CLIParameter::new_number(prefix, delimiter, *value),
                );
                map
            },
        )
    }

    #[inline]
    pub fn new_bool(prefix: &str, value: bool) -> CLIParameter {
        CLIParameter::Bool {
            prefix: prefix.to_owned(),
            value,
        }
    }

    #[inline]
    pub fn new_bools(prefix: &str, values: &[(&str, bool)]) -> HashMap<String, CLIParameter> {
        values.iter().fold(
            HashMap::<String, CLIParameter>::new(),
            |mut map, (key, value)| {
                map.insert(
                    key.to_owned().to_owned(),
                    CLIParameter::new_bool(prefix, *value),
                );
                map
            },
        )
    }

    /// Check if the current parameter matches the other parameter by prefix and
    /// delimiter but not value.
    #[inline]
    pub fn matches(&self, other: &CLIParameter) -> bool {
        match self {
            CLIParameter::String {
                prefix,
                delimiter,
                ..
            } => match other {
                CLIParameter::String {
                    prefix: other_prefix,
                    delimiter: other_delimiter,
                    ..
                } => prefix == other_prefix && delimiter == other_delimiter,
                _ => false,
            },
            CLIParameter::Number {
                prefix,
                delimiter,
                ..
            } => match other {
                CLIParameter::Number {
                    prefix: other_prefix,
                    delimiter: other_delimiter,
                    ..
                } => prefix == other_prefix && delimiter == other_delimiter,
                _ => false,
            },
            CLIParameter::Bool {
                prefix, ..
            } => match other {
                CLIParameter::Bool {
                    prefix: other_prefix,
                    ..
                } => prefix == other_prefix,
                _ => false,
            },
        }
    }

    #[inline]
    pub fn to_parameter_string(&self, name: &str) -> String {
        match self {
            CLIParameter::String {
                prefix,
                delimiter,
                value,
            } => {
                format!("{}{}{}{}", prefix, name, delimiter, value)
            },
            CLIParameter::Number {
                prefix,
                delimiter,
                value,
            } => {
                format!("{}{}{}{}", prefix, name, delimiter, value)
            },
            CLIParameter::Bool {
                prefix,
                value,
            } => {
                if *value {
                    format!("{}{}", prefix, name)
                } else {
                    String::new()
                }
            },
        }
    }

    #[inline]
    pub fn to_string_pair(&self, name: &str) -> (Option<String>, Option<String>) {
        match self {
            CLIParameter::String {
                prefix,
                value,
                delimiter,
            } => match delimiter.as_str() {
                " " => (Some(format!("{}{}", prefix, name)), Some(value.to_owned())),
                _ => (
                    Some(format!("{}{}{}{}", prefix, name, delimiter, value)),
                    None,
                ),
            },
            CLIParameter::Number {
                prefix,
                value,
                delimiter,
            } => match delimiter.as_str() {
                " " => (Some(format!("{}{}", prefix, name)), Some(value.to_string())),
                _ => (
                    Some(format!("{}{}{}{}", prefix, name, delimiter, value)),
                    None,
                ),
            },
            CLIParameter::Bool {
                prefix,
                value,
            } => {
                if *value {
                    (Some(format!("{}{}", prefix, name)), None)
                } else {
                    (None, None)
                }
            },
        }
    }

    #[inline]
    pub fn to_string_value(&self) -> String {
        match self {
            CLIParameter::String {
                value, ..
            } => value.clone(),
            CLIParameter::Number {
                value, ..
            } => value.to_string(),
            CLIParameter::Bool {
                value, ..
            } => value.to_string(),
        }
    }
}
