---
name: feedback-findings-epic-parent
description: When tracking review findings, add them to the parent epic of the reviewed task — only use a "Review Findings" fallback epic if no parent exists
type: gotcha
---

Review findings (suggestions, warnings from code review) must be created as children of the same epic that the reviewed task belongs to.

**Why:** The findings are contextually related to the epic's scope. Creating a separate "Review Findings" epic fragments tracking and loses the connection to the work area. This mistake was corrected multiple times.

**How to apply:** When filing review findings, check the reviewed task's parent epic first and use that as the destination. Only fall back to creating/using a generic "Review Findings" epic if the task has no parent epic at all.
