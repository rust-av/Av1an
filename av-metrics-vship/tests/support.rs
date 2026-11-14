//! Shared synthetic fixture self-checks register here, not in each consumer.
#![cfg(test)]

mod common;
#[path = "common/tests.rs"]
mod fixtures;
