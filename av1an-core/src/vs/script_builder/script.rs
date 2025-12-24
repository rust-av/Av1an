use std::{collections::BTreeMap, fmt::Write};

pub type ModuleName = String;
pub type PackageName = String;
pub type ModuleAlias = Option<String>;
pub type VariableName = String;
pub type VariableValue = String;
pub type OutputIndex = u8;

pub type Imports = BTreeMap<PackageName, BTreeMap<ModuleName, ModuleAlias>>;
pub type Lines = Vec<Line>;
pub type Outputs = BTreeMap<OutputIndex, VariableName>;

pub struct VapourSynthScript {
    pub imports: Imports,
    pub lines:   Lines,
    pub outputs: Outputs,
}

impl Default for VapourSynthScript {
    #[inline]
    fn default() -> Self {
        let mut imports = BTreeMap::new();
        let mut vs_modules = BTreeMap::new();
        vs_modules.insert("core".to_owned(), None);
        imports.insert("vapoursynth".to_owned(), vs_modules);

        Self {
            imports,
            lines: Vec::new(),
            outputs: BTreeMap::new(),
        }
    }
}

impl std::fmt::Display for VapourSynthScript {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // let mut sorted_imports: Vec<_> = self.imports.iter().collect();
        // sorted_imports.sort_by(|(package_a, modules_a), (package_b, modules_b)| {
        //     if package_a == package_b {
        //         std::cmp::Ordering::Equal
        //     } else {
        //         package_a.cmp(package_b)
        //     }
        // });
        // for (package, modules) in &sorted_imports {
        for (package, modules) in self.imports.iter() {
            f.write_str("from ")?;
            f.write_str(package)?;
            f.write_str(" import ")?;

            let first_module = modules.iter().next();
            if let Some((module, alias)) = first_module {
                f.write_str(module)?;
                if let Some(alias) = alias {
                    f.write_str(&format!(" as {}", alias))?;
                }
            }
            for (module, alias) in modules.iter().skip(1) {
                f.write_str(&format!(", {}", module))?;
                if let Some(alias) = alias {
                    f.write_str(&format!(" as {}", alias))?;
                }
            }
            f.write_char('\n')?;
        }
        f.write_char('\n')?;
        for line in &self.lines {
            match line {
                Line::Comment(comment) => {
                    f.write_str("# ")?;
                    f.write_str(comment)?;
                    f.write_char('\n')?;
                },
                Line::Term(term) => {
                    f.write_str(term)?;
                    f.write_char('\n')?;
                },
                Line::TermWithComment(term, comment) => {
                    f.write_str(term)?;
                    f.write_str(" # ")?;
                    f.write_str(comment)?;
                    f.write_char('\n')?;
                },
                Line::Expression(variable_name, variable_value) => {
                    f.write_str(variable_name)?;
                    f.write_str(" = ")?;
                    f.write_str(variable_value)?;
                    f.write_char('\n')?;
                },
                Line::ExpressionWithComment(variable_name, variable_value, comment) => {
                    f.write_str(variable_name)?;
                    f.write_str(" = ")?;
                    f.write_str(variable_value)?;
                    f.write_str(" # ")?;
                    f.write_str(comment)?;
                    f.write_char('\n')?;
                },
            }
        }
        f.write_char('\n')?;
        for (index, name) in self.outputs.iter() {
            let line = format!("{}.set_output(index = {})", name, index);
            f.write_str(&line)?;
            f.write_char('\n')?;
        }
        f.write_char('\n')?;
        Ok(())
    }
}

impl VapourSynthScript {
    #[inline]
    pub fn add_lines(&mut self, lines: Lines) -> &mut Self {
        self.lines.extend(lines);
        self
    }

    #[inline]
    pub fn add_imports(&mut self, imports: Imports) -> &mut Self {
        for (package, modules) in imports {
            let existing_modules = self.imports.entry(package).or_default();
            existing_modules.extend(modules);
        }
        self
    }

    #[inline]
    pub fn add_outputs(&mut self, outputs: Outputs) -> &mut Self {
        self.outputs.extend(outputs);
        self
    }
}
pub enum Line {
    /// Will be prefixed with `# `
    Comment(String),
    /// Can be a function call
    ///
    /// example: `foo(bar, baz)`
    Term(String),
    /// Can be a function call and a comment appended
    ///
    /// example: `foo(bar, baz) # comment`
    TermWithComment(String, String),
    /// Declare variables and their values on the same line
    ///
    /// example: `foo = bar`
    Expression(VariableName, VariableValue),
    /// Declare variables and their values on the same line with a comment
    ///
    /// example: `foo = bar # comment`
    ExpressionWithComment(VariableName, VariableValue, String),
}
