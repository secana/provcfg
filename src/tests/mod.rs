#![allow(dead_code)]
// Imports and fixtures are shared across feature-gated tests; which ones are
// live depends on the enabled feature set.
#![allow(unused_imports)]

mod attributes;
mod basics;
mod defaults;
mod fixtures;
mod sources;
mod walk_and_serialize;
