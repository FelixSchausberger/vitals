//! TWHS algorithm implementation.
//!
//! The algorithm proceeds in 7 steps:
//! 1. Temporal frecency per event
//! 2. Severity weight multiplication → raw penalty
//! 3. Cascade attribution → adjusted penalty
//! 4. Baseline-relative resource penalties
//! 5. Total burden
//! 6. Exponential score mapping
//! 7. Transparency layer (burden share percentages)

pub mod cascade;
pub mod frecency;
pub mod normalization;
pub mod resources;
