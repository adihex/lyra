#!/usr/bin/env node
// Apply QA failure-catalog updates from qa-results/skill-updates.json.
// Dependency-free (node stdlib only). Used by the qa.yml workflow for
// failure_learning=open_pr (and auto_commit).
//
// JSON format: [{ file, section, action: append|replace, content }]
// - file: repo-relative path to the skill markdown file
// - section: markdown heading text to find (e.g. "Known Failure Modes")
// - action: append (add after the section's last numbered item) or
//   replace (replace the entire section body up to the next ## heading)
// - content: exact markdown to insert
//
// Exit 0 with "APPLIED n" / "NO_UPDATES" / "NO_FILE" messages for the
// workflow to gate on. Never throws on missing file: warns and skips.

import fs from 'node:fs';
import path from 'node:path';

function main() {
  const input = process.argv[2] || 'qa-results/skill-updates.json';
  if (!fs.existsSync(input)) {
    console.log('NO_FILE: skill-updates.json not present, nothing to apply');
    return;
  }
  let entries;
  try {
    entries = JSON.parse(fs.readFileSync(input, 'utf8'));
  } catch (e) {
    console.log(`NO_UPDATES: could not parse ${input}: ${e.message}`);
    return;
  }
  if (!Array.isArray(entries) || entries.length === 0) {
    console.log('NO_UPDATES: empty skill-updates list');
    return;
  }
  let applied = 0;
  for (const e of entries) {
    if (!e || typeof e.file !== 'string' || typeof e.section !== 'string'
        || typeof e.content !== 'string') {
      console.log(`SKIP: malformed entry ${JSON.stringify(e)}`);
      continue;
    }
    const filePath = path.resolve(e.file);
    if (!fs.existsSync(filePath)) {
      console.log(`SKIP: file not found ${e.file}`);
      continue;
    }
    const action = e.action === 'replace' ? 'replace' : 'append';
    const text = fs.readFileSync(filePath, 'utf8');
    const lines = text.split('\n');
    const idx = lines.findIndex((l) => l.replace(/^#+\s*/, '').trim() === e.section.trim());
    if (idx === -1) {
      console.log(`SKIP: section "${e.section}" not found in ${e.file}`);
      continue;
    }
    // End of section: next line starting with '## ' (same or higher level)
    // after idx, or EOF.
    let end = lines.length;
    for (let i = idx + 1; i < lines.length; i++) {
      if (/^##\s/.test(lines[i])) { end = i; break; }
    }
    let next;
    if (action === 'replace') {
      next = [...lines.slice(0, idx + 1), '', e.content.trimEnd(), '', ...lines.slice(end)];
    } else {
      // Append after the section body, keeping a blank line separator.
      const body = lines.slice(idx + 1, end);
      while (body.length && body[body.length - 1].trim() === '') body.pop();
      const head = lines.slice(0, idx + 1);
      const tail = lines.slice(end);
      next = [...head, ...body, '', e.content.trimEnd(), '', ...tail];
    }
    fs.writeFileSync(filePath, next.join('\n'));
    applied++;
    console.log(`APPLIED: ${e.file} [${e.section}] (${action})`);
  }
  console.log(applied === 0 ? 'NO_UPDATES: nothing applied' : `APPLIED ${applied}`);
}

main();
