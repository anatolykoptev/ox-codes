//! Svelte directive parsing: extracts named identifiers from on:event,
//! use:action, bind:target, transition:fn, in:fn, out:fn, animate:fn,
//! let:name, class:name, style:prop attributes.

/// Svelte directive prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectiveKind {
    EventHandler,  // on:event
    Action,        // use:action
    Binding,       // bind:target
    Transition,    // transition:fn, in:fn, out:fn
    Animation,     // animate:fn
    Let,           // let:name
    Class,         // class:name
    Style,         // style:prop
    Unknown,
}

/// A parsed Svelte attribute directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub kind: DirectiveKind,
    /// The name after the colon (e.g. "click" for on:click).
    pub name: String,
    /// Modifiers split by `|` (e.g. ["preventDefault", "once"]).
    pub modifiers: Vec<String>,
}

/// Parses `attribute_name` text like `"on:click|preventDefault"` into a
/// [`Directive`].  Returns `None` for plain attributes (`class`, `id`, etc.).
pub fn parse_directive(attribute_name: &str) -> Option<Directive> {
    let (prefix, rest) = attribute_name.split_once(':')?;
    let mut parts = rest.splitn(2, '|');
    let name = parts.next().unwrap_or("").to_string();
    if name.is_empty() {
        return None;
    }
    let modifiers: Vec<String> = parts
        .next()
        .map(|m| m.split('|').map(str::to_string).collect())
        .unwrap_or_default();

    let kind = match prefix {
        "on" => DirectiveKind::EventHandler,
        "use" => DirectiveKind::Action,
        "bind" => DirectiveKind::Binding,
        "transition" | "in" | "out" => DirectiveKind::Transition,
        "animate" => DirectiveKind::Animation,
        "let" => DirectiveKind::Let,
        "class" => DirectiveKind::Class,
        "style" => DirectiveKind::Style,
        _ => DirectiveKind::Unknown,
    };
    Some(Directive { kind, name, modifiers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_directive_event_handler() {
        let d = parse_directive("on:click").unwrap();
        assert_eq!(d.kind, DirectiveKind::EventHandler);
        assert_eq!(d.name, "click");
        assert!(d.modifiers.is_empty());
    }

    #[test]
    fn test_parse_directive_action() {
        let d = parse_directive("use:drag").unwrap();
        assert_eq!(d.kind, DirectiveKind::Action);
        assert_eq!(d.name, "drag");
    }

    #[test]
    fn test_parse_directive_binding() {
        let d = parse_directive("bind:value").unwrap();
        assert_eq!(d.kind, DirectiveKind::Binding);
        assert_eq!(d.name, "value");
    }

    #[test]
    fn test_parse_directive_with_modifiers() {
        let d = parse_directive("on:click|preventDefault|once").unwrap();
        assert_eq!(d.kind, DirectiveKind::EventHandler);
        assert_eq!(d.name, "click");
        assert_eq!(d.modifiers, vec!["preventDefault", "once"]);
    }

    #[test]
    fn test_parse_directive_non_directive_returns_none() {
        assert!(parse_directive("class").is_none());
        assert!(parse_directive("id").is_none());
        assert!(parse_directive("href").is_none());
    }

    #[test]
    fn test_parse_directive_transition() {
        let d = parse_directive("transition:fade").unwrap();
        assert_eq!(d.kind, DirectiveKind::Transition);
        assert_eq!(d.name, "fade");
    }

    #[test]
    fn test_parse_directive_in_out() {
        let d = parse_directive("in:fly").unwrap();
        assert_eq!(d.kind, DirectiveKind::Transition);
        let d2 = parse_directive("out:fly").unwrap();
        assert_eq!(d2.kind, DirectiveKind::Transition);
    }

    #[test]
    fn test_parse_directive_animate() {
        let d = parse_directive("animate:flip").unwrap();
        assert_eq!(d.kind, DirectiveKind::Animation);
    }

    #[test]
    fn test_parse_directive_empty_name_returns_none() {
        assert!(parse_directive("on:").is_none());
    }
}
