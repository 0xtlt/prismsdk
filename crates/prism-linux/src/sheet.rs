//! The stylesheet of Prism's nodes: one rule per distinct style, shared by
//! every node whose style writes the same CSS (class `s<n>`). Reloading the
//! sheet restyles every widget, so it only reloads for a new rule: the rows
//! of a long list share a few rules, and rows scrolling in reuse them. No
//! GTK types, so it is tested on every host.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};

/// Stands for a node's own selector (`.p<id>`) in a rule's template.
const SELF: &str = "\u{1}";

/// A node's new class after [`Sheet::set`].
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// Its class before, if it had one.
    pub old: Option<u32>,
    pub new: u32,
    /// The sheet has a new rule: it must be loaded again.
    pub added: bool,
}

#[derive(Default)]
pub struct Sheet {
    /// A rule's template → its class.
    classes: HashMap<String, u32>,
    /// Each class in use: its template and how many nodes have it.
    used: HashMap<u32, (String, usize)>,
    /// Each class's rule, in creation order.
    rules: BTreeMap<u32, String>,
    nodes: HashMap<u64, u32>,
    next: u32,
}

impl Sheet {
    /// Gives node `id` the rule `rule`, written for its own class `.p<id>`
    /// (the node's selector, nested or not; no other node's).
    pub fn set(&mut self, id: u64, rule: &str) -> Change {
        let template = rule.replace(&format!(".p{id}"), SELF);
        let current = self.nodes.get(&id).copied();
        if let Some(n) = current
            && self.used.get(&n).is_some_and(|(t, _)| *t == template)
        {
            return Change {
                old: current,
                new: n,
                added: false,
            };
        }
        let old = self.remove(id);
        let (new, added) = match self.classes.get(&template) {
            Some(&n) => (n, false),
            None => {
                let n = self.next;
                self.next += 1;
                self.rules
                    .insert(n, template.replace(SELF, &format!(".s{n}")));
                self.classes.insert(template.clone(), n);
                self.used.insert(n, (template, 0));
                (n, true)
            }
        };
        if let Some((_, users)) = self.used.get_mut(&new) {
            *users += 1;
        }
        self.nodes.insert(id, new);
        Change { old, new, added }
    }

    /// Node `id` is gone (or changes class): its class, if it had one. A
    /// rule no node uses leaves with the next load; meanwhile it matches
    /// nothing, as class numbers are never reused.
    pub fn remove(&mut self, id: u64) -> Option<u32> {
        let n = self.nodes.remove(&id)?;
        let unused = self.used.get_mut(&n).is_some_and(|(_, users)| {
            *users -= 1;
            *users == 0
        });
        if unused && let Some((template, _)) = self.used.remove(&n) {
            self.classes.remove(&template);
            self.rules.remove(&n);
        }
        Some(n)
    }

    /// The whole stylesheet.
    pub fn css(&self) -> String {
        self.rules.values().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_with_the_same_style_share_one_rule() {
        let mut s = Sheet::default();
        let a = s.set(7, ".p7{color:red;}\n.p7:hover{color:blue;}\n");
        assert_eq!(
            a,
            Change {
                old: None,
                new: 0,
                added: true
            }
        );
        // Another node, the same CSS: its class, no new rule.
        let b = s.set(12, ".p12{color:red;}\n.p12:hover{color:blue;}\n");
        assert_eq!(
            b,
            Change {
                old: None,
                new: 0,
                added: false
            }
        );
        assert_eq!(s.css(), ".s0{color:red;}\n.s0:hover{color:blue;}\n");
        // The same style again: nothing changes.
        assert!(!s.set(7, ".p7{color:red;}\n.p7:hover{color:blue;}\n").added);
    }

    #[test]
    fn a_new_style_moves_the_node_and_unused_rules_leave() {
        let mut s = Sheet::default();
        s.set(1, ".p1{color:red;}\n");
        s.set(2, ".p2{color:red;}\n");
        let c = s.set(1, ".p1{color:green;}\n");
        assert_eq!(
            c,
            Change {
                old: Some(0),
                new: 1,
                added: true
            }
        );
        // Node 2 still uses the red rule.
        assert_eq!(s.css(), ".s0{color:red;}\n.s1{color:green;}\n");
        assert_eq!(s.remove(2), Some(0));
        assert_eq!(s.css(), ".s1{color:green;}\n");
        // Class numbers are never reused.
        assert_eq!(s.set(3, ".p3{color:red;}\n").new, 2);
        assert_eq!(s.remove(9), None);
    }

    #[test]
    fn only_the_node_own_selector_is_shared() {
        let mut s = Sheet::default();
        // Nested and group selectors keep their other classes.
        s.set(
            4,
            ".prism-group-row:hover .p4{opacity:0.5;}\n.p4 > button{color:red;}\n",
        );
        assert_eq!(
            s.css(),
            ".prism-group-row:hover .s0{opacity:0.5;}\n.s0 > button{color:red;}\n"
        );
    }
}
