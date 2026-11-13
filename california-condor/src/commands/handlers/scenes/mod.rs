//! View and edit scenes.
//!
//! The list is addressed by an index and every action reads or changes the
//! scenes that index selects. An index is a single scene (`1`), an
//! inclusive range (`0-4`), a list of either (`1-2,8-10,16`), or
//! `-1`/`all`/`every` for the whole list ([`SceneSelection`]).
//!
//! The `info` / `set` / `reset` triple matches every other command's
//! configuration editing (see [`crate::commands::handlers::config`]), but the
//! index sits between the command and the action, so this handler drives the
//! three itself and adds the two list-shaped actions: `delete` and `add`. The
//! scene-list rules every command that edits `condor.scenes` shares live in
//! [`crate::scenes`].

mod action;
mod edit;
mod report;

#[cfg(test)]
mod tests;

use std::path::Path;

use action::Placement;
pub use action::{ScenesAction, ScenesConfigError, ScenesKey, ScenesSet};
use anyhow::{Result, bail};
use edit::{add_scene, apply_set, remove_scenes, reset_key, reset_scene};
use report::{aggregate, frame_summary, label, report_totals, resolve, save_edit, selection_value};
use serde_json::Value;

use crate::{
    commands::{
        config::PropertyChange,
        handlers::{
            config::{announce, print_json},
            load_configuration,
        },
        is_verbose,
    },
    scenes::{SceneSelection, ScenesError, require_scene},
};

/// The name every `av1an scenes` change is reported under.
const SUBJECT: &str = "Scenes";

/// `av1an scenes [INDEX] [info|set|reset|delete|add]`.
///
/// Without an action the addressed scenes are printed, the same as `info`.
pub fn config(
    config_path: Option<&Path>,
    index: Option<&SceneSelection>,
    action: Option<&ScenesAction>,
) -> Result<()> {
    match action {
        None => info(config_path, index, None),
        Some(ScenesAction::Info { key }) => info(config_path, index, key.as_ref()),
        Some(ScenesAction::Set { setting }) => set(config_path, index, setting),
        Some(ScenesAction::Reset { key }) => reset(config_path, index, key.as_ref()),
        Some(ScenesAction::Delete) => delete(config_path, index),
        Some(ScenesAction::Add {
            start,
            end,
            append,
            prepend,
            after,
        }) => {
            let placement = match (append, prepend, after) {
                (true, ..) => Placement::Append,
                (_, true, _) => Placement::Prepend,
                (.., Some(after)) => Placement::After(*after),
                _ => Placement::Sorted,
            };
            add(config_path, *start, *end, placement)
        },
    }
}

/// `av1an scenes [INDEX] info [KEY]`.
fn info(
    config_path: Option<&Path>,
    index: Option<&SceneSelection>,
    key: Option<&ScenesKey>,
) -> Result<()> {
    let (configuration, _) = load_configuration(config_path)?;
    let scenes = &configuration.condor.scenes;
    let indices = resolve(scenes, index)?;
    let single = index.is_some() && indices.len() == 1;
    let value = selection_value(scenes, &indices, key, single)?;
    print_json(&value)?;
    report_totals(scenes, &indices);

    Ok(())
}

/// `av1an scenes INDEX set ...`.
fn set(
    config_path: Option<&Path>,
    index: Option<&SceneSelection>,
    setting: &ScenesSet,
) -> Result<()> {
    let Some(index) = index else {
        bail!(ScenesConfigError::SetNeedsIndex);
    };
    let (mut configuration, path) = load_configuration(config_path)?;
    if configuration.condor.scenes.is_empty() {
        bail!(ScenesError::NoScenes);
    }
    let indices = index.resolve(configuration.condor.scenes.len())?;
    let before = configuration.condor.scenes.clone();
    let mut name = None;
    let mut values = Vec::with_capacity(indices.len());
    for &index in &indices {
        let (changed, value) = apply_set(&mut configuration, index, setting)?;
        name = Some(changed);
        values.push(value);
    }
    let change = aggregate(&label(&indices), name, values);
    save_edit(&configuration, &path, &before)?;
    announce(SUBJECT, &change, false);
    report_totals(&configuration.condor.scenes, &indices);

    Ok(())
}

/// `av1an scenes [INDEX] reset [KEY]`.
///
/// When neither index nor key is provided, clear the whole list.
fn reset(
    config_path: Option<&Path>,
    index: Option<&SceneSelection>,
    key: Option<&ScenesKey>,
) -> Result<()> {
    let (mut configuration, path) = load_configuration(config_path)?;
    if index.is_none() && key.is_none() {
        if configuration.condor.scenes.is_empty() {
            return Ok(());
        }
        let before = configuration.condor.scenes.clone();
        configuration.condor.scenes.clear();
        let change = PropertyChange::new("section", &configuration.condor.scenes)?;
        save_edit(&configuration, &path, &before)?;
        announce(SUBJECT, &change, true);
        println!("Run `av1an detect-scenes` to rebuild the scene list.");
        // The summary covers the list this run cleared, so it is taken from
        // the scenes as they were found rather than from the empty list.
        report_totals(&before, &resolve(&before, None)?);

        return Ok(());
    }
    if configuration.condor.scenes.is_empty() {
        bail!(ScenesError::NoScenes);
    }
    let indices = resolve(&configuration.condor.scenes, index)?;
    let before = configuration.condor.scenes.clone();
    let global = configuration.condor.encoder.clone();
    let mut name = None;
    let mut values = Vec::with_capacity(indices.len());
    for &index in &indices {
        match key {
            // Without a key the scene itself is reset, which its label names
            // already.
            None => values.push(reset_scene(&mut configuration, index, &global)?),
            Some(key) => {
                let (changed, value) = reset_key(&mut configuration, index, *key, &global)?;
                name = Some(changed);
                values.push(value);
            },
        }
    }
    let change = aggregate(&label(&indices), name, values);
    save_edit(&configuration, &path, &before)?;
    announce(SUBJECT, &change, true);
    report_totals(&configuration.condor.scenes, &indices);

    Ok(())
}

/// `av1an scenes INDEX delete`.
fn delete(config_path: Option<&Path>, index: Option<&SceneSelection>) -> Result<()> {
    let Some(index) = index else {
        bail!(ScenesConfigError::DeleteNeedsIndex);
    };
    let (mut configuration, path) = load_configuration(config_path)?;
    if configuration.condor.scenes.is_empty() {
        bail!(ScenesError::NoScenes);
    }
    let indices = index.resolve(configuration.condor.scenes.len())?;
    let before = configuration.condor.scenes.clone();
    let removed = remove_scenes(&mut configuration, &indices)?;
    save_edit(&configuration, &path, &before)?;
    println!(
        "Deleted {}: {}",
        label(&indices),
        frame_summary(&before, &indices)
    );
    if is_verbose() {
        let removed = if removed.len() == 1 {
            removed.into_iter().next().expect("one scene")
        } else {
            Value::Array(removed)
        };
        print_json(&removed)?;
    }
    // The summary covers the scenes this run removed, which only `before`
    // still holds.
    report_totals(&before, &indices);

    Ok(())
}

/// `av1an scenes add START END`.
fn add(config_path: Option<&Path>, start: usize, end: usize, placement: Placement) -> Result<()> {
    let (mut configuration, path) = load_configuration(config_path)?;
    let before = configuration.condor.scenes.clone();
    let position = add_scene(&mut configuration, start, end, placement)?;
    let scene = require_scene(&configuration.condor.scenes, position)?;
    save_edit(&configuration, &path, &before)?;
    println!("Added scene {position}: {start}-{end}");
    if is_verbose() {
        let change = PropertyChange::new("add", scene)?;
        print_json(&change.value)?;
    }
    report_totals(&configuration.condor.scenes, &[position]);

    Ok(())
}
