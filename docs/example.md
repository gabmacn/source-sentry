---
title: Example
description: A document whose sources Source Sentry tracks through its own frontmatter.
source-sentry:
  reviewed:
    at: 2026-11-07T15:02:44Z
    by: 'Bruno Costa'
  classes: ['internal-docs']
  owners: ['platform']
  sources:
    - path: '../crates/core/src/lib.rs'
      oid: 'sha1:0000000000000000000000000000000000000000'
      reviewed:
        at: 2026-11-07T14:32:05Z
        by: 'Bruno Costa'
    - path: '../README.md'
---

# Example document

Source Sentry reads only the `source-sentry` key of this frontmatter. When it
updates that key, it leaves every other key, the text and the line endings
exactly as they were.

The document and its first source have been reviewed. The second source has
not, so this document is stale until someone reviews it.
