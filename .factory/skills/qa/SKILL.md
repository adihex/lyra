---
name: qa
description: >
  Run QA tests for Lyra. Analyzes git diff to determine affected areas,
  runs configured test flows with multiple personas, and generates diff-targeted tests.
  Uses droid-control for native desktop interactions and direct CLI execution for the lyra CLI.
  Use when testing PRs, releases, or smoke testing local builds.
---

# QA Orchestrator

**SCOPE: This skill performs manual/functional QA only -- verifying that the application actually works by interacting with it as a real user would (desktop app, CLI, socket protocol). Do NOT run or report on CI checks, linting, typecheck, unit tests, or any static analysis. Those are handled by separate workflows.**

## Step 1: Load Configuration

Read `.factory/skills/qa/config.yaml` for environment URLs, credentials, personas, and app definitions.

For the desktop app, invoke the `droid-control` skill before interacting with the app. Follow its routing tables:

- Load the target driver selected for the app (native desktop via `desktop-control`).
- Use the Capture and Verify stages on every run.
- Load Compose only when both `video_evidence: true` and `droid_control.compose: true`.
- When `droid_control.compose: false`, never invoke Compose. Keep raw captures and text snapshots as evidence.

Treat `droid_control.compose` as a runtime toggle. Read it on every run; do not cache the install-time answer.
Do not repeat the resolved boolean in this file or any app sub-skill. Keep all branch instructions conditional on the config value so changing the config is sufficient.

The CLI app needs no interactive driver: it is driven by direct process execution (`cargo run -p lyra-cli -- ...` or the built binary) against a socket, with `--json` output for assertions.

## Step 2: Determine Target Environment

Use the default_target from config unless the user specifies a different environment.
Lyra has exactly one environment: `local` (locally built `.app` + CLI on the dev machine).
There are no path restrictions.

## Step 3: Analyze Git Diff

Run `git diff` to determine what changed. Map changed files to apps using the path_patterns in config.yaml.

Files that don't match ANY app's path_patterns (e.g., `.factory/skills/**`, `docs/**`, `.github/**`, `README.md`, `BLUEPRINT.md`) are NOT associated with any app. Do NOT run app test flows for them.

For each affected app:

- Run ONLY that app's flows from its module file
- Generate ADDITIONAL targeted tests based on the specific changes in the diff

For apps NOT affected by the diff:

- Do NOT load or run their module. Do NOT run their flows. Do NOT run their pre-flight checks. They are completely out of scope.
- Do NOT test the desktop app if only CLI/IPC files changed. Do NOT test the CLI if only SwiftUI files changed. The diff determines scope, period.

If NO app is affected by the diff (e.g., docs-only, CI-only, or config-only changes), report as INCONCLUSIVE: "No app code changed -- QA not applicable for this diff." Do NOT run any app flows.

LAN remote pairing is explicitly out of QA scope. If the diff only touches pairing handshake code with no other user-visible effect, report INCONCLUSIVE with that reason.

## Step 4: Pre-flight Checks (app-specific only)

Run pre-flight checks ONLY for the apps that are affected by the diff. For example:

- CLI binary builds (`cargo build -p lyra-cli`) → only if the CLI app is affected
- Desktop `.app` builds (`make app`) → only if the desktop app is affected
- A display is available (needed for any desktop-control interaction) → only if the desktop app is affected
- `droid-control@factory-plugins` active → for the desktop app (interactive)
- Compose prerequisites from the droid-control skill → only when `droid_control.compose: true`

When Compose is on, let the droid-control Compose stage resolve its own plugin root. If the plugin's `remotion/node_modules` is missing, install from its lockfile in that `remotion` directory before rendering. Never install Remotion dependencies when Compose is off.

**Desktop app testing in CI:** GitHub-hosted macOS runners have no display. When no display is available, report ALL qa-desktop flows as BLOCKED: "No display available -- cannot drive the native UI." Do NOT fall back to anything else. CLI flows still run normally.

Do NOT run pre-flight checks for apps that are NOT affected. If a pre-flight check fails for an affected app, report it as BLOCKED with the specific error and remediation steps -- but still proceed with other affected apps.

## Step 5: Execute Diff-Relevant Flows Only

For each app that IS affected by the diff, read its sub-skill from `.factory/skills/qa-<app-name>/SKILL.md`.

The sub-skill contains a MENU of available test flows. You must:

1. Read the diff carefully and identify which flows are relevant to the change
2. Run those flows PLUS any adjacent flows that verify the change integrates correctly (e.g., if a new CLI subcommand is added, test that it appears in `--help` and `capabilities`, that the CLI starts, that invalid args return exit code 4)
3. Do NOT run completely unrelated flows (e.g., if the diff only adds a CLI command, do NOT test EQ curves, visualizations, or the desktop pet)
4. If no existing flow covers the change, write a NEW ad-hoc test that directly verifies the changed behavior
5. Do NOT run unit tests, lint, typecheck, or any automated test suite. This is manual/functional QA -- interact with the app as a real user would.

## Step 6: Evidence Capture

After each significant test step, capture evidence. Use **text snapshots as primary evidence** -- they render inline in the PR comment with no image hosting issues.

For the CLI app (direct-exec):

- Run the built binary (or `cargo run -p lyra-cli --`) with `--json` for machine-readable assertions.
- Embed command + exit code + relevant JSON output directly in the report as fenced code blocks with a descriptive label.
- When `video_evidence: true`, there is nothing to record for CLI runs (no visual output). Text transcripts ARE the evidence. Never fabricate a video for CLI flows.

For the desktop app (desktop-control):

- Use the `droid-control` native-desktop route; let its desktop-control atom own mechanics and its Capture atom own the recording lifecycle.
- Capture accessibility/UI-tree snapshots as text evidence through the routed droid-control driver.
- Save screenshot files to `./qa-results/$RUN_ID/` for the artifact upload.
- On GitHub with `video_evidence: false` (or when an upload fails): do NOT embed `![image](url)` markdown in the report; artifact and repository URLs cannot be displayed inline in GitHub PR comments. Instead, mention the filename and note that it's available in the downloadable artifacts.
- On GitHub with `video_evidence: true`: screenshots use `![name](asset-url)` image markdown; videos go on their own bare line (see video rules below).

**Video evidence (when `video_evidence: true` in config.yaml):**

Desktop flows only. CLI flows never produce video. Then either keep the raw recording or run droid-control Compose based on the config:

1. Follow the droid-control Capture atom for recording start, interactions, recording stop, and raw-output verification.
2. Record exactly one video per flow. Do NOT record one video for the whole run.
3. If `droid_control.compose: false`, keep the driver's raw output (the native driver's recording format). Upload a raw recording only when the format is a verified playable video.
4. If `droid_control.compose: true`, hand the raw recording to the droid-control Compose atom and render `./qa-results/$RUN_ID/<flow-slug>.mp4`.
   - Use a `single` layout and pass the literal `"preset": "factory"` to Compose.
   - The `factory` preset is implementation-owned, not configuration. Do not offer, select, or permit any other Compose preset in the questionnaire, YAML, generated skills, or runtime prompts, including `factory-hero`, `macos`, `minimal`, `hero`, and `presentation`.
   - Trim dead time, add a concise title card, and use annotations only when they clarify the proof.
   - Keep the final edited video at 60 seconds or less.
   - Verify the MP4 with the Compose atom's finalization checks before upload.
5. Verify the selected evidence file exists and is non-empty. If capture or Compose fails, retry that stage once; if it still fails, fall back to text evidence and retain any raw capture in job artifacts.
6. Upload each recording and verify the response, via the user-attachments endpoint (undocumented; verify against the live response):

   ```bash
   REPO_ID="${REPO_ID:-$(gh api "repos/$GITHUB_REPOSITORY" --jq .id)}"
   curl -sS --fail-with-body --request POST \
     --header "Authorization: Bearer $QA_EVIDENCE_TOKEN" \
     --header "Content-Type: application/octet-stream" \
     --header "Accept: application/json" \
     --header "X-GitHub-Api-Version: 2022-11-28" \
     "https://uploads.github.com/user-attachments/assets?name=<evidence-file>&content_type=<url-encoded-content-type>&repository_id=$REPO_ID" \
     --data-binary "@./qa-results/$RUN_ID/<evidence-file>" \
   | jq -e '.url'
   ```

   Token rule: this endpoint accepts user tokens only, specifically **classic** PATs with `repo` scope (OAuth user tokens also work). Live-verified failure signatures:

   - **404** with a valid `repository_id`: installation token (the Actions `GITHUB_TOKEN` or a GitHub App token). These can never upload; use the PAT secret.
   - **403 `Resource not accessible by personal access token`**: fine-grained PAT. Rejected outright; re-mint as a classic PAT.
   - **403 `Resource protected by organization SAML enforcement`**: classic PAT not yet authorized for the org. Fix at github.com/settings/tokens -> "Configure SSO" next to the token -> Authorize. The token value does not change, so the stored secret needs no update.

   Use the verified raw video's filename and MIME type when Compose is off. Use `<flow-slug>.mp4` with `video/mp4` when Compose is on.

7. Embed rules on GitHub: place the returned `url` value on its own line with nothing else on that line. NEVER wrap it in `![name](url)`; image-markdown wrapping stops GitHub from rendering the player entirely. Verified player extensions: `.webm` and `.mp4` (GitHub's docs also list `.mov`).
8. Screenshots (`.png`) can be uploaded the same way. On GitHub, screenshots DO use image markdown (`![name](asset-url)`); only videos need the bare line.
9. If an upload fails, fall back to text evidence and note the video filename is available in job artifacts. Never embed a link you have not verified came back from the upload API.

Evidence quality rules:

- Focus on the RELEVANT content. Trim snapshots to the meaningful part.
- Label each snapshot clearly: what it shows and why it matters for the test.
- NEVER embed broken image links. If you can't verify an image URL will resolve, use text evidence instead.
- The workflow uploads all files in `./qa-results/` as a downloadable artifact -- reference that for visual evidence.

## Step 7: Test Quality Gate

TEST QUALITY REQUIREMENTS:

1. CHANGE-SPECIFIC FIRST. Prioritize tests that directly verify the behavioral change in the diff. At least half your tests should be testing the new/changed feature itself.
2. INTEGRATION TESTS ARE VALID. Tests that verify the change integrates correctly with existing features are good (e.g., new command shows in --help, fuzzy search finds it, CLI starts without errors). These are NOT smoke tests -- they verify the change didn't break integration points.
3. NO UNRELATED FLOWS. Do NOT test features completely unrelated to the diff (e.g., don't test EQ curves when only queue code changed, don't test the desktop pet when only the CLI changed).
4. NO AUTOMATED TEST SUITES. Do NOT run cargo test or any CI-style checks. This is manual/functional QA only.
5. NEGATIVE TESTS. Include at least 1 test verifying error handling or boundary conditions related to the change (e.g., invalid seek spec → exit code 4, unknown method → `unknown_method` error code).
6. INTERACTIVE TESTING. Test by actually interacting with the app as a real user would (drive the desktop UI, run the real CLI against a real socket).
7. INCONCLUSIVE IF UNSURE. If you cannot articulate what the PR changes, mark as INCONCLUSIVE rather than PASS.
8. CLEANUP AFTER EVERY TEST. Per config.yaml cleanup instructions: restore queue, EQ, temp dirs, and sidecars after each mutating test. Verify cleanup happened (e.g., `queue list` empty after the test that added tracks).

## Step 8: Handle Failures

**Never silently skip a flow.** If a flow cannot complete, report it as BLOCKED with what was tried and how the user can fix it. Then continue to the next flow -- never abort the entire run for a single failure.

## Step 9: Generate Report

Generate the report at `./qa-results/report.md` using `.factory/skills/qa/REPORT-TEMPLATE.md`.

The report MUST follow the template in `.factory/skills/qa/REPORT-TEMPLATE.md`. Key rules:

- Start with `## QA Report` heading followed by the test results table
- Result column MUST use emojis: :white_check_mark: PASS, :x: FAIL, :no_entry: BLOCKED, :warning: FLAKY, :grey_question: INCONCLUSIVE
- Keep it CONCISE. The table + a short "Action Required" section (if any) + collapsed screenshots = the entire report.
- Do NOT include: "Behavioral Change Summary", "Blocked Flows" prose, "Info" metadata table, or verbose explanations of what the diff does. The reviewer already knows that.
- Do NOT report setup/prerequisite steps (building, startup, launching) as test rows. Those are means to an end, not test cases. Only report rows that verify actual user-facing behavior or the specific behavioral change from the diff.
- Put ALL evidence in a single collapsed `<details>` block
- For CLI evidence: embed command + exit code + JSON output as labeled fenced code blocks.
- For desktop evidence: embed UI-tree snapshots as text. Reference screenshot filenames for visual proof (available in downloadable artifacts). With `video_evidence: true`: put each returned user-attachments URL on its own line (bare, never wrapped in `![]()`) so it renders an inline player; screenshots use `![name](asset-url)`. For any flow whose upload failed, use the text-first rules instead.

## Step 10: Suggest Skill Updates (Failure Learning)

After generating the report, check if any BLOCKED or FAIL results revealed a **testing environment insight** that would help future QA runs succeed. This is about learning how the testing environment works, NOT about fixing bad selectors or skill typos.

**Good suggestions** (environment/workflow knowledge):

- "The dev server requires `make app`, not `cargo run` -- the SwiftUI shell only exists in the .app bundle"
- "CoreAudio hog-mode tests need a real output device -- headless CI has none, always BLOCKED there"
- "The control socket path comes from `LYRA_SOCKET` env or well-known paths -- document which one the test used"

**Bad suggestions** (skill bugs, not environment insights -- do NOT suggest these):

- "Selector foo doesn't exist" -- that's a skill bug, fix it directly
- "The button text changed from X to Y" -- that's expected from the PR diff

Format as a table with severity, collapsible fix prompts, and a count in the heading:

## Suggested Skill Updates (N issues found)

| #   | Severity        | File     | Issue               | Fix Prompt                                                                           |
| --- | --------------- | -------- | ------------------- | ------------------------------------------------------------------------------------ |
| 1   | <emoji> <level> | `<file>` | <short description> | <details><summary>Copy</summary><br>`<full droid prompt to fix the issue>`</details> |

**Severity levels:**

- `🔴 Breaking` -- Causes test failures every run (wrong URL, wrong auth method, missing required step)
- `🟡 Degraded` -- Causes intermittent failures or suboptimal behavior (timing issues, rate limits, locale assumptions)
- `🔵 Info` -- New knowledge that improves future runs but doesn't cause failures (new UI pattern, new endpoint)

Each Fix Prompt must be a self-contained instruction that Droid can execute directly when pasted.

Do NOT suggest updates for failures already covered in Known Failure Modes, bad selectors, or expected behavior changes from the PR. If no genuinely new environment insights were discovered, omit this section entirely.

Read the `failure_learning` field from config.yaml to determine the strategy:

- This project uses `open_pr`: include the table in the report AND write a `qa-results/skill-updates.json` file so the workflow can apply the edits outside the sandbox. The workflow handles PR creation -- the agent just writes the JSON.

**`skill-updates.json` format** (only for `auto_commit` or `open_pr`):

```json
[
  {
    "file": ".factory/skills/qa-desktop/SKILL.md",
    "section": "Known Failure Modes",
    "action": "append",
    "content": "6. **Example quirk.** One-line description plus the workaround."
  }
]
```

Fields:

- `file`: relative path to the skill file to edit
- `section`: the markdown heading to find (e.g., `Known Failure Modes`, `Authentication Method`)
- `action`: `append` (add after the section's last item) or `replace` (replace the entire section content)
- `content`: the exact markdown to insert

The workflow will parse this file and apply the edits to the actual repo files, then open a draft PR depending on the mode.
