use std::collections::{HashMap, HashSet};

use pole_specification::v1::{
    match_string::{MatchStringType, ValueType},
    Api, MatchString,
};

use crate::plugins::router::rule::helper::match_label_value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ApiMatchInput {
    pub method: Option<String>,
    pub path: Option<String>,
}

impl ApiMatchInput {
    pub(crate) fn new(method: Option<String>, path: Option<String>) -> Self {
        Self { method, path }
    }
}

#[derive(Clone)]
struct Candidate<T> {
    order: usize,
    value: T,
}

#[derive(Clone)]
struct FallbackCandidate<T> {
    order: usize,
    api: Api,
    value: T,
}

struct PathTrie<T> {
    root: PathTrieNode<T>,
}

struct PathTrieNode<T> {
    children: HashMap<char, PathTrieNode<T>>,
    values: Vec<Candidate<T>>,
}

impl<T> Default for PathTrie<T> {
    fn default() -> Self {
        Self {
            root: PathTrieNode::default(),
        }
    }
}

impl<T> Default for PathTrieNode<T> {
    fn default() -> Self {
        Self {
            children: HashMap::new(),
            values: Vec::new(),
        }
    }
}

impl<T: Clone> PathTrie<T> {
    fn insert(&mut self, path: &str, candidate: Candidate<T>) {
        let mut node = &mut self.root;
        for ch in path.chars() {
            node = node.children.entry(ch).or_default();
        }
        node.values.push(candidate);
    }

    fn exact_candidates(&self, path: &str, output: &mut Vec<Candidate<T>>) {
        let mut node = &self.root;
        for ch in path.chars() {
            let Some(next) = node.children.get(&ch) else {
                return;
            };
            node = next;
        }
        output.extend(node.values.iter().cloned());
    }
}

struct MethodApiIndex<T> {
    pathless: Vec<Candidate<T>>,
    exact_paths: PathTrie<T>,
    fallback_paths: Vec<FallbackCandidate<T>>,
}

impl<T> Default for MethodApiIndex<T> {
    fn default() -> Self {
        Self {
            pathless: Vec::new(),
            exact_paths: PathTrie::default(),
            fallback_paths: Vec::new(),
        }
    }
}

impl<T: Clone> MethodApiIndex<T> {
    fn insert_pathless(&mut self, candidate: Candidate<T>) {
        self.pathless.push(candidate);
    }

    fn insert_exact_path(&mut self, path: &str, candidate: Candidate<T>) {
        self.exact_paths.insert(path, candidate);
    }

    fn insert_fallback_path(&mut self, api: Api, candidate: Candidate<T>) {
        self.fallback_paths.push(FallbackCandidate {
            order: candidate.order,
            api,
            value: candidate.value,
        });
    }

    fn candidates(&self, input: &ApiMatchInput, output: &mut Vec<Candidate<T>>) {
        output.extend(self.pathless.iter().cloned());
        if let Some(path) = input.path.as_deref() {
            self.exact_paths.exact_candidates(path, output);
        }
        output.extend(
            self.fallback_paths
                .iter()
                .filter(|candidate| api_matches(input, &candidate.api))
                .map(|candidate| Candidate {
                    order: candidate.order,
                    value: candidate.value.clone(),
                }),
        );
    }
}

pub(crate) struct ApiMatchIndex<T> {
    match_all: Vec<Candidate<T>>,
    wildcard_method: MethodApiIndex<T>,
    methods: HashMap<String, MethodApiIndex<T>>,
}

impl<T: Clone> ApiMatchIndex<T> {
    pub(crate) fn new<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (T, &'a [Api])>,
        T: 'a,
    {
        let mut index = Self {
            match_all: Vec::new(),
            wildcard_method: MethodApiIndex::default(),
            methods: HashMap::new(),
        };

        for (order, (value, apis)) in entries.into_iter().enumerate() {
            let candidate = Candidate { order, value };
            if apis.is_empty() {
                index.match_all.push(candidate);
                continue;
            }
            for api in apis {
                index.insert_api(api, candidate.clone());
            }
        }

        index
    }

    pub(crate) fn candidates(&self, input: &ApiMatchInput) -> Vec<T> {
        let mut candidates = self.match_all.clone();
        self.wildcard_method.candidates(input, &mut candidates);
        if let Some(method) = input.method.as_deref() {
            if let Some(method_index) = self.methods.get(method) {
                method_index.candidates(input, &mut candidates);
            }
        }

        candidates.sort_by_key(|candidate| candidate.order);
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter_map(|candidate| {
                if seen.insert(candidate.order) {
                    Some(candidate.value)
                } else {
                    None
                }
            })
            .collect()
    }

    fn insert_api(&mut self, api: &Api, candidate: Candidate<T>) {
        let method_index = if api.method.is_empty() || api.method == "*" {
            &mut self.wildcard_method
        } else {
            self.methods.entry(api.method.clone()).or_default()
        };

        match exact_text_path(api.path.as_ref()) {
            PathIndexKind::Any => method_index.insert_pathless(candidate),
            PathIndexKind::Exact(path) => method_index.insert_exact_path(path, candidate),
            PathIndexKind::Fallback => method_index.insert_fallback_path(api.clone(), candidate),
        }
    }
}

enum PathIndexKind<'a> {
    Any,
    Exact(&'a str),
    Fallback,
}

fn exact_text_path(path: Option<&MatchString>) -> PathIndexKind<'_> {
    let Some(path) = path else {
        return PathIndexKind::Any;
    };
    if path.r#type() == MatchStringType::Exact && path.value_type() == ValueType::Text {
        PathIndexKind::Exact(path.value.as_str())
    } else {
        PathIndexKind::Fallback
    }
}

pub(crate) fn api_matches(input: &ApiMatchInput, api: &Api) -> bool {
    if !api.method.is_empty() && api.method != "*" {
        if input.method.as_deref() != Some(api.method.as_str()) {
            return false;
        }
    }
    if let Some(path_rule) = &api.path {
        let Some(actual_path) = input.path.as_ref() else {
            return false;
        };
        if !match_label_value(path_rule, actual_path.clone()) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_api(method: &str, path: &str) -> Api {
        Api {
            method: method.to_string(),
            path: Some(MatchString {
                r#type: MatchStringType::Exact.into(),
                value: path.to_string(),
                value_type: ValueType::Text.into(),
            }),
            ..Api::default()
        }
    }

    fn regex_api(method: &str, path: &str) -> Api {
        Api {
            method: method.to_string(),
            path: Some(MatchString {
                r#type: MatchStringType::Regex.into(),
                value: path.to_string(),
                value_type: ValueType::Text.into(),
            }),
            ..Api::default()
        }
    }

    #[test]
    fn api_index_uses_exact_path_trie_to_reduce_candidates() {
        let mut entries = (0..1_000)
            .map(|idx| (idx, vec![exact_api("GET", &format!("/orders/{idx}"))]))
            .collect::<Vec<_>>();
        entries.push((1_001, vec![exact_api("GET", "/orders/target")]));
        entries.push((1_002, vec![regex_api("GET", r"^/orders/target$")]));
        entries.push((1_003, Vec::new()));

        let index = ApiMatchIndex::new(
            entries
                .iter()
                .map(|(value, apis)| (*value, apis.as_slice())),
        );

        let candidates = index.candidates(&ApiMatchInput::new(
            Some("GET".to_string()),
            Some("/orders/target".to_string()),
        ));

        assert_eq!(candidates, vec![1_001, 1_002, 1_003]);
    }

    #[test]
    fn api_match_requires_path_when_api_path_is_configured() {
        let input = ApiMatchInput::new(Some("GET".to_string()), None);

        assert!(!api_matches(&input, &exact_api("GET", "/orders/42")));
    }

    #[test]
    fn api_match_keeps_empty_api_list_as_match_all() {
        let index = ApiMatchIndex::new(
            [(7, Vec::new())]
                .iter()
                .map(|(value, apis)| (*value, apis.as_slice())),
        );

        assert_eq!(index.candidates(&ApiMatchInput::new(None, None)), vec![7]);
    }

    #[test]
    fn api_index_deduplicates_entries_with_multiple_matching_apis() {
        let entries = [(
            7,
            vec![
                exact_api("GET", "/orders/42"),
                regex_api("GET", r"^/orders/\d+$"),
            ],
        )];
        let index = ApiMatchIndex::new(
            entries
                .iter()
                .map(|(value, apis)| (*value, apis.as_slice())),
        );

        let candidates = index.candidates(&ApiMatchInput::new(
            Some("GET".to_string()),
            Some("/orders/42".to_string()),
        ));

        assert_eq!(candidates, vec![7]);
    }
}
