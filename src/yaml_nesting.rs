//! A streaming pre-pass over YAML text with the tokenizer `yaml_serde` itself
//! uses (libyaml-rs). It stops at the first collection nested deeper than the
//! bound. The tokenizer's work per token grows with the number of open flow
//! collections (`[`, `{`), so a deep `[[[[...` inside the 1 MiB file bound
//! would otherwise take minutes; with nesting bounded the parse stays linear.
//! Only event types and positions are read; nothing is kept.

use std::mem::MaybeUninit;

use libyaml_rs as sys;

/// Owns an initialised libyaml parser on the heap and frees it on drop.
struct ParserGuard(Box<MaybeUninit<sys::yaml_parser_t>>);

impl ParserGuard {
    fn as_mut_ptr(&mut self) -> *mut sys::yaml_parser_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for ParserGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is only built after `yaml_parser_initialize`
        // succeeded on this allocation, and it is deleted exactly once.
        unsafe { sys::yaml_parser_delete(self.as_mut_ptr()) }
    }
}

/// The 1-based line of the first sequence or mapping nested more than `max`
/// levels deep, or `None` when nesting stays within `max`. A syntax error
/// ends the pre-pass with `None`: the full parser reports it.
pub fn first_line_deeper_than(text: &str, max: usize) -> Option<u64> {
    let mut slot = Box::new(MaybeUninit::<sys::yaml_parser_t>::uninit());
    // SAFETY: `slot` is a valid, heap-pinned allocation for one parser.
    if !unsafe { sys::yaml_parser_initialize(slot.as_mut_ptr()) }.ok {
        return None;
    }
    let mut parser = ParserGuard(slot);
    let input = text.as_bytes();
    // SAFETY: the parser is initialised; `input` outlives it (the guard is
    // dropped before this function returns) and is not moved meanwhile.
    unsafe {
        sys::yaml_parser_set_encoding(parser.as_mut_ptr(), sys::YAML_UTF8_ENCODING);
        sys::yaml_parser_set_input_string(parser.as_mut_ptr(), input.as_ptr(), input.len() as _);
    }
    let mut depth = 0usize;
    loop {
        let mut event = MaybeUninit::<sys::yaml_event_t>::uninit();
        // SAFETY: on success `yaml_parser_parse` fully initialises `event`,
        // which is read and then deleted once; on failure it is not touched.
        let (kind, line) = unsafe {
            if !sys::yaml_parser_parse(parser.as_mut_ptr(), event.as_mut_ptr()).ok {
                return None;
            }
            let event = event.as_mut_ptr();
            let seen = ((*event).type_, (*event).start_mark.line);
            sys::yaml_event_delete(event);
            seen
        };
        match kind {
            sys::YAML_SEQUENCE_START_EVENT | sys::YAML_MAPPING_START_EVENT => {
                depth += 1;
                if depth > max {
                    return Some(line.saturating_add(1));
                }
            }
            sys::YAML_SEQUENCE_END_EVENT | sys::YAML_MAPPING_END_EVENT => {
                depth = depth.saturating_sub(1);
            }
            sys::YAML_STREAM_END_EVENT => return None,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_is_measured_in_flow_and_block_style() {
        let flow = |n: usize| format!("a: {}1{}\n", "[".repeat(n), "]".repeat(n));
        // `a:` is one mapping; the flow sequences add one level each.
        assert_eq!(first_line_deeper_than(&flow(31), 32), None);
        assert_eq!(first_line_deeper_than(&flow(32), 32), Some(1));
        let mut block = String::new();
        for level in 0..33 {
            block.push_str(&format!("{}k{level}:\n", "  ".repeat(level)));
        }
        block.push_str(&format!("{}v: 1\n", "  ".repeat(33)));
        assert_eq!(first_line_deeper_than(&block, 32), Some(33));
        assert_eq!(first_line_deeper_than("a: [1, {b: 'c]]]'}]\n", 32), None);
        // A syntax error is left to the full parser.
        assert_eq!(first_line_deeper_than("a: [\n", 32), None);
    }
}
