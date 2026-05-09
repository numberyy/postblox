<!-- qa-report -->
## QA Report

| # | Test Case | App | Persona | Result | Notes |
|---|-----------|-----|---------|--------|-------|
{{TEST_ROWS}}

Result values: :white_check_mark: PASS, :x: FAIL, :no_entry: BLOCKED, :warning: FLAKY, :grey_question: INCONCLUSIVE

{{#if ACTIONABLE_ITEMS}}

### Action Required

{{ACTIONABLE_ITEMS}}
{{/if}}

{{#if SKILL_UPDATES}}

## Suggested Skill Updates ({{SKILL_UPDATE_COUNT}} issues found)

| # | Severity | File | Issue | Fix Prompt |
|---|----------|------|-------|------------|
{{SKILL_UPDATES}}

Severity legend: :red_circle: Breaking, :yellow_circle: Degraded, :large_blue_circle: Info
{{/if}}

<details>
<summary>Snapshots & Evidence</summary>

{{EVIDENCE}}

</details>

---
_Run ID: `{{RUN_ID}}` · Workflow: [{{WORKFLOW_RUN_NAME}}]({{WORKFLOW_RUN_URL}}) · Artifacts: [download]({{ARTIFACT_URL}})_
