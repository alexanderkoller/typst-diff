// Cross-references with #set heading(numbering: ...).
// Adding a section in new.typ shifts counter values of Methods (2→3) and Results (3→4).
// Expected: reference text in Introduction changes from "Section 2" to "Section 3", etc.

#set heading(numbering: "1.")

= Introduction <intro>

The experimental setup is described in @methods. Outcomes appear in @results.

= Methods <methods>

Samples were prepared at three temperatures. See @results for the data.

= Results <results>

All results are consistent with the predictions from @intro.
