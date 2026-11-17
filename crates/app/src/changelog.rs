//! the Changelog window: the repository's CHANGELOG.md, read only. there is no markdown
//! library; headings are bold, list items are indented and the rest is plain text.

use crate::view::Node;

/// the changelog as it was when this build was made.
pub const TEXT: &str = include_str!("../../../CHANGELOG.md");

/// the nodes for `text`: a heading line (`#`, `##`, `###`) is a bold line with a gap before it,
/// a `- ` item is indented with a bullet, and anything else is a line of text.
pub fn nodes<M>(text: &str) -> Vec<Node<M>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(title) = line.strip_prefix('#') {
            if !out.is_empty() {
                out.push(Node::Gap(6.0));
            }
            out.push(Node::Heading(title.trim_start_matches('#').trim().into()));
        } else if let Some(item) = line.trim_start().strip_prefix("- ") {
            out.push(Node::Text(format!("    \u{2022} {item}")));
        } else {
            out.push(Node::Text(line.into()));
        }
    }
    out
}

/// the nodes of the changelog this build carries.
pub fn view<M>() -> Vec<Node<M>> {
    nodes(TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_are_bold_and_items_are_indented_bullets() {
        let nodes: Vec<Node<()>> =
            nodes("# Changelog\n\n## [1.0.0]\n\n### Added\n\n- A thing (#1)\n");
        let heads: Vec<_> = nodes
            .iter()
            .filter_map(|n| match n {
                Node::Heading(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(heads, ["Changelog", "[1.0.0]", "Added"]);
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Text(t) if t == "    \u{2022} A thing (#1)")));
    }

    #[test]
    fn the_real_changelog_is_embedded_and_has_its_title() {
        assert!(TEXT.starts_with("# Changelog"));
        assert!(matches!(view::<()>().first(), Some(Node::Heading(t)) if t == "Changelog"));
    }
}
