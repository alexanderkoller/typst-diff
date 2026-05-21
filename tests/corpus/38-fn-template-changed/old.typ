// Template function: == heading, no prefix.
// Change: heading level drops from 2 to 1 AND a bold "Summary:" prefix is added.
// The args are identical. Expected: "Summary:" appears as Insert in each body paragraph.

#let section(title, body) = [
  == #title

  #body
]

#section("Overview")[The overview provides context for the reader.]

#section("Details")[The details section expands on the overview with specifics.]

#section("Summary")[The summary recaps all the key findings and conclusions.]
