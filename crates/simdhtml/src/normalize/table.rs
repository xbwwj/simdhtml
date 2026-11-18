//! Table wrapper insertion for the common structural cases.

use super::{EventSink, TagRef, find_last, tag_code};
use crate::tagcode::TagCode;
use crate::tokenizer::Tokenized;

pub(super) fn insert_table_wrappers(
    tokens: &Tokenized,
    stack: &mut Vec<TagRef>,
    incoming: TagCode,
    position: u32,
    sink: &mut impl EventSink,
) {
    // Only table-structure start tags can trigger a wrapper.  Checking this
    // first keeps the common non-table start tag off the stack scan below.
    if !matches!(
        incoming,
        TagCode::Tr | TagCode::Td | TagCode::Th | TagCode::Col
    ) {
        return;
    }

    let Some(table_pos) = find_last(stack, tokens, &[TagCode::Table], &[]) else {
        return;
    };
    let has = |targets: &[TagCode]| {
        stack[table_pos + 1..]
            .iter()
            .any(|&tag| targets.contains(&tag_code(tokens, tag)))
    };

    let mut synth = |name: &'static str, stack: &mut Vec<TagRef>| {
        sink.start_synthetic(name, position..position);
        stack.push(TagRef::Synthetic(name));
    };

    match incoming {
        TagCode::Tr => {
            if !has(&[TagCode::Thead, TagCode::Tbody, TagCode::Tfoot]) {
                synth("tbody", stack);
            }
        }
        TagCode::Td | TagCode::Th => {
            if !has(&[TagCode::Tr]) {
                if !has(&[TagCode::Thead, TagCode::Tbody, TagCode::Tfoot]) {
                    synth("tbody", stack);
                }
                synth("tr", stack);
            }
        }
        TagCode::Col if !has(&[TagCode::Colgroup]) => {
            synth("colgroup", stack);
        }
        _ => {}
    }
}
