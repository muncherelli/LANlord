//! the Changelog window: the repository's CHANGELOG.md, read only. there is no markdown
//! library; headings are bold, list items are indented and the rest is plain text.

use crate::view::Node;

/// the changelog as it was when this build was made.
pub const TEXT: &str = include_str!("../../../CHANGELOG.md");

/// `[text](url)` as just `text`: there is nowhere to follow a link to in this window.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let linked = rest[open..].find("](").and_then(|mid| {
            let end = rest[open + mid..].find(')')?;
            Some((open + mid, open + mid + end))
        });
        match linked {
            Some((mid, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&rest[open + 1..mid]);
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// the nodes for `text`, from the first version heading on (the title and the template's intro
/// are left out): a heading line (`##`, `###`) is a bold line with a gap before it, a `- ` item
/// is a bullet, and anything else is a line of text. links show as their text.
pub fn nodes<M>(text: &str) -> Vec<Node<M>> {
    let mut out = Vec::new();
    for line in text.lines().skip_while(|l| !l.starts_with("## ")) {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(title) = line.strip_prefix('#') {
            if !out.is_empty() {
                out.push(Node::Gap(6.0));
            }
            out.push(Node::Heading(plain(title.trim_start_matches('#').trim())));
        } else if let Some(item) = line.trim_start().strip_prefix("- ") {
            out.push(Node::Bullet(plain(item)));
        } else {
            out.push(Node::Text(plain(line)));
        }
    }
    out
}

/// the nodes of the changelog this build carries.
pub fn view<M>() -> Vec<Node<M>> {
    nodes(crate::pin::changelog())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_at_the_first_version_headings_bold_with_bullets_and_no_link_syntax() {
        let nodes: Vec<Node<()>> = nodes(
            "# Changelog\n\nSee [Keep a Changelog](https://x.test/a).\n\n## [1.0.0]\n\n### Added\n\n- A [thing](https://x.test) (#1)\n",
        );
        let heads: Vec<_> = nodes
            .iter()
            .filter_map(|n| match n {
                Node::Heading(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(heads, ["[1.0.0]", "Added"]);
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Bullet(t) if t == "A thing (#1)")));
        assert!(!nodes.iter().any(|n| matches!(n, Node::Text(_))));
    }

    #[test]
    fn the_real_changelog_has_no_markdown_links_or_template_intro() {
        let all: Vec<Node<()>> = view();
        assert!(matches!(all.first(), Some(Node::Heading(t)) if t.contains("Unreleased")));
        for n in &all {
            if let Node::Text(t) | Node::Bullet(t) | Node::Heading(t) = n {
                assert!(!t.contains("]("), "{t}");
            }
        }
    }
}
