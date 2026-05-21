// Uses Typst state to track and display progress values.
// In new.typ: the progress percentages change (25->30, 50->65, 75->80).
// Expected: modifications in the progress-display paragraphs.

#let progress = state("progress", 0)

= Project Status Report

== Phase 1: Research

The research phase is now complete.

#progress.update(25)
#context [Current overall progress: #progress.get()%.]

This phase involved literature review, stakeholder interviews,
and definition of the evaluation criteria.

== Phase 2: Design

The design phase is underway.

#progress.update(50)
#context [Current overall progress: #progress.get()%.]

Initial wireframes have been reviewed and approved.
The technical specification document is in draft.

== Phase 3: Implementation

Implementation has begun on schedule.

#progress.update(75)
#context [Current overall progress: #progress.get()%.]

The core module is functional. Integration testing starts next week.

== Phase 4: Review

Review and sign-off are pending completion of implementation.

#context [Final recorded progress: #progress.get()%.]
