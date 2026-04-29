#!/usr/bin/env python3
with open("scorer/src/types.rs", "r") as f:
    content = f.read()

# Fix must_use on occurrence_datetimes
content = content.replace(
    "pub fn occurrence_datetimes(&self) -> Vec<OffsetDateTime> {",
    "#[must_use]\n    pub fn occurrence_datetimes(&self) -> Vec<OffsetDateTime> {"
)

# Fix must_use on status
old_status = 'pub fn status(&self) -> &\'static str {'
new_status = "#[must_use]\n    pub fn status(&self) -> &'static str {"
content = content.replace(old_status, new_status)

# Fix doc_markdown issues - F_i, λ, age_k, P_raw, weight, frecency, cascade_i, R_j, u_j, etc.
content = content.replace("F_i = Σ exp(-λ · age_k)", "`F_i` = Σ exp(-λ · `age_k`)")
content = content.replace("P_raw = weight × frecency", "`P_raw` = `weight` × `frecency`")
content = content.replace("P_adj = P_raw × (1 - attribution)", "`P_adj` = `P_raw` × (1 - `attribution`)")
content = content.replace("R_j = r_max × sigmoid", "`R_j` = `r_max` × sigmoid")
content = content.replace("(u - μ) / (σ × k)", "(`u` - `μ`) / (`σ` × `k`)")
content = content.replace("T = issue_burden + resource_burden", "`T` = `issue_burden` + `resource_burden`")
content = content.replace("Σ alpha_j × R_j", "Σ `alpha_j` × `R_j`")

with open("scorer/src/types.rs", "w") as f:
    f.write(content)

print("Fixed types.rs")
