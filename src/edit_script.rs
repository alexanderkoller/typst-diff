//! Shared edit-script construction for ordered semantic children.
//!
//! Slots, child sequences, and later structured container children all need the
//! same ordered script shape: equal runs, deletes anchored into the new side,
//! inserts, and replacements. This module keeps the Myers plumbing in one place
//! so callers provide only keys and edit semantics.

use similar::{Algorithm, DiffOp, capture_diff_slices};
use std::hash::Hash;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditScriptMode {
    SameShape,
    Lcs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditScriptOp {
    Equal {
        old_index: usize,
        new_index: usize,
        len: usize,
    },
    Delete {
        old_index: usize,
        old_len: usize,
        new_index: usize,
    },
    Insert {
        new_index: usize,
        new_len: usize,
    },
    Replace {
        old_index: usize,
        old_len: usize,
        new_index: usize,
        new_len: usize,
    },
}

pub(crate) fn build_edit_script<K>(
    old_keys: &[K],
    new_keys: &[K],
    mode: EditScriptMode,
) -> Vec<EditScriptOp>
where
    K: Eq + Hash + Ord,
{
    match mode {
        EditScriptMode::SameShape => same_shape_script(old_keys.len(), new_keys.len()),
        EditScriptMode::Lcs => lcs_script(old_keys, new_keys),
    }
}

fn same_shape_script(old_len: usize, new_len: usize) -> Vec<EditScriptOp> {
    let paired = old_len.min(new_len);
    let mut ops = Vec::new();
    if paired > 0 {
        ops.push(EditScriptOp::Equal {
            old_index: 0,
            new_index: 0,
            len: paired,
        });
    }
    if old_len > paired {
        ops.push(EditScriptOp::Delete {
            old_index: paired,
            old_len: old_len - paired,
            new_index: paired,
        });
    }
    if new_len > paired {
        ops.push(EditScriptOp::Insert {
            new_index: paired,
            new_len: new_len - paired,
        });
    }
    ops
}

fn lcs_script<K>(old_keys: &[K], new_keys: &[K]) -> Vec<EditScriptOp>
where
    K: Eq + Hash + Ord,
{
    capture_diff_slices(Algorithm::Myers, old_keys, new_keys)
        .into_iter()
        .map(|op| match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => EditScriptOp::Equal {
                old_index,
                new_index,
                len,
            },
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => EditScriptOp::Delete {
                old_index,
                old_len,
                new_index,
            },
            DiffOp::Insert {
                new_index, new_len, ..
            } => EditScriptOp::Insert { new_index, new_len },
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => EditScriptOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_shape_pairs_by_position() {
        assert_eq!(
            build_edit_script(&["a", "b"], &["x", "y"], EditScriptMode::SameShape),
            vec![EditScriptOp::Equal {
                old_index: 0,
                new_index: 0,
                len: 2
            }]
        );
    }

    #[test]
    fn lcs_keeps_matching_runs_and_inserts() {
        assert_eq!(
            build_edit_script(&["a", "c"], &["a", "b", "c"], EditScriptMode::Lcs),
            vec![
                EditScriptOp::Equal {
                    old_index: 0,
                    new_index: 0,
                    len: 1
                },
                EditScriptOp::Insert {
                    new_index: 1,
                    new_len: 1
                },
                EditScriptOp::Equal {
                    old_index: 1,
                    new_index: 2,
                    len: 1
                }
            ]
        );
    }
}
