// Uses @preview/showybox:2.0.4 from Typst Universe.
// Box titles and body text changed from old.typ.

#import "@preview/showybox:2.0.4": showybox

= Project Overview

#showybox(
  title: "Goal",
  frame: (
    border-color: blue,
    title-color: blue.lighten(30%),
  ),
)[
  The goal of this project is to model pedestrian flow in suburban areas
  and identify bottlenecks that reduce throughput at peak hours.
]

#showybox(
  title: "Scope",
  frame: (
    border-color: green,
    title-color: green.lighten(30%),
  ),
)[
  The study covers three suburban districts, including all signalized intersections
  and pedestrian crossings within a ten-kilometer radius of the transit hub.
]

#showybox(
  title: "Timeline",
  frame: (
    border-color: orange,
    title-color: orange.lighten(30%),
  ),
)[
  Phase one will conclude by the end of February.
  Phase two deliverables are due in May, with a final report in August.
]
