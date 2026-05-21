// Template function: = heading (level 1), adds "Summary: " prefix.
// Args are identical to old version.

#let section(title, body) = [
  = #title

  *Summary:* #body
]

#section("Overview")[The overview provides context for the reader.]

#section("Details")[The details section expands on the overview with specifics.]

#section("Summary")[The summary recaps all the key findings and conclusions.]
