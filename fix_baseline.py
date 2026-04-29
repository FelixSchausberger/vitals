#!/usr/bin/env python3
# Fix baseline.rs

with open("scorer/src/baseline.rs", "r") as f:
    content = f.read()

# Fix must_use attributes
content = content.replace("pub fn variance(&self) -> f64 {", "#[must_use]\n    pub fn variance(&self) -> f64 {")
content = content.replace("pub fn std(&self) -> f64 {", "#[must_use]\n    pub fn std(&self) -> f64 {")
content = content.replace("pub fn detect() -> Self {", "#[must_use]\n    pub fn detect() -> Self {")
content = content.replace("pub fn resource_priors(self) -> ResourcePriors {", "#[must_use]\n    pub fn resource_priors(self) -> ResourcePriors {")
content = content.replace("pub fn new() -> Self {", "#[must_use]\n    pub fn new() -> Self {")
content = content.replace("pub fn from_system_profile(profile: SystemProfile, seed_weight: u64) -> Self {", "#[must_use]\n    pub fn from_system_profile(profile: SystemProfile, seed_weight: u64) -> Self {")

# Fix manual_range_patterns for 33 | 34 | 35 | 36 -> 33..=36
content = content.replace("33 | 34 | 35 | 36 => Self::Embedded,", "33..=36 => Self::Embedded,")

# Fix cast_precision_loss - add allow at module level for WelfordState impl
old_impl = "impl WelfordState {"
new_impl = "impl WelfordState {\n    #![allow(clippy::cast_precision_loss)]"
content = content.replace(old_impl, new_impl)

with open("scorer/src/baseline.rs", "w") as f:
    f.write(content)

print("Fixed baseline.rs")