use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn invalid(message: impl Into<String>) -> Error {
    Error::new("invalid-automation", message)
}
pub(crate) fn text(value: &str) -> Result<(), Error> {
    if value.len() > MAX_TEXT_BYTES || value.trim().is_empty() {
        return Err(invalid(
            "text/identity must be nonblank and at most 1024 bytes",
        ));
    }
    Ok(())
}
struct Budget(usize);
impl Budget {
    fn spend(&mut self, amount: usize) -> Result<(), Error> {
        self.0 = self
            .0
            .checked_add(amount)
            .filter(|v| *v <= MAX_WORK)
            .ok_or_else(|| Error::new("resource-limit", "validation work exceeds 4096 items"))?;
        Ok(())
    }
}
fn length(len: usize, max: usize) -> Result<(), Error> {
    if len > max {
        return Err(Error::new(
            "resource-limit",
            "collection cardinality exceeds format-v1 limit",
        ));
    }
    Ok(())
}
fn admit_strings(values: &[String], budget: &mut Budget) -> Result<(), Error> {
    length(values.len(), MAX_ITEMS)?;
    budget.spend(values.len())?;
    for s in values {
        text(s)?;
    }
    Ok(())
}
fn admit_value(v: &Value, budget: &mut Budget) -> Result<(), Error> {
    budget.spend(1)?;
    match v {
        Value::Text(s) => text(s),
        Value::TextSet(s) => admit_strings(s, budget),
        _ => Ok(()),
    }
}
fn admit_key(key: &FactKey) -> Result<(), Error> {
    text(&key.name)?;
    if let Some(s) = &key.subject {
        text(s)?;
    }
    Ok(())
}
fn admit_condition(c: &Condition, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::new("resource-limit", "condition nesting exceeds 32"));
    }
    budget.spend(1)?;
    match c {
        Condition::All { conditions } | Condition::Any { conditions } => {
            length(conditions.len(), MAX_ITEMS)?;
            for c in conditions {
                admit_condition(c, depth + 1, budget)?;
            }
        }
        Condition::Not { condition } => admit_condition(condition, depth + 1, budget)?,
        Condition::Fact { key, predicate } => {
            admit_key(key)?;
            budget.spend(1)?;
            match predicate {
                Predicate::Eq { value } | Predicate::Ne { value } => admit_value(value, budget)?,
                Predicate::In { values } => {
                    length(values.len(), MAX_ITEMS)?;
                    for v in values {
                        admit_value(v, budget)?;
                    }
                }
                Predicate::Contains { value } => text(value)?,
                Predicate::ContainsAll { values } | Predicate::ContainsAny { values } => {
                    admit_strings(values, budget)?
                }
                _ => {}
            }
        }
    }
    Ok(())
}
pub(crate) fn admit_action(a: &Action) -> Result<(), Error> {
    if let Action::Select {
        profile_id,
        overrides,
    } = a
    {
        if let Some(s) = profile_id {
            text(s)?;
        }
        length(overrides.len(), MAX_ITEMS)?;
        for o in overrides {
            match o {
                Override::Send { source, mix, .. } => {
                    text(source)?;
                    text(mix)?;
                }
                Override::Mix { mix, .. } => text(mix)?,
                Override::Output { output, .. } => text(output)?,
            }
        }
    }
    Ok(())
}
// Full structural admission precedes duplicate scans, catalog lookup and graph traversal.
// A 50,000-item set is rejected by len(), even when supplied directly by a Rust caller.
fn admit(graph: &Graph, catalog: &[FactDefinition], snapshot: &Snapshot) -> Result<(), Error> {
    length(graph.nodes.len(), MAX_NODES)?;
    length(catalog.len(), MAX_FACTS)?;
    length(snapshot.facts.len(), MAX_FACTS)?;
    text(&graph.id)?;
    text(&graph.name)?;
    text(&graph.root)?;
    text(&snapshot.epoch)?;
    let mut budget = Budget(0);
    for d in catalog {
        budget.spend(1)?;
        text(&d.name)?;
    }
    for f in &snapshot.facts {
        budget.spend(1)?;
        admit_key(&f.key)?;
        text(&f.provider)?;
        if let Some(reason) = &f.reason {
            text(reason)?;
        }
        if let Some(value) = &f.value {
            admit_value(value, &mut budget)?;
        }
    }
    for n in &graph.nodes {
        budget.spend(1)?;
        text(n.id())?;
        match n {
            Node::Decision {
                condition,
                on_true,
                on_false,
                on_unknown,
                ..
            } => {
                text(on_true)?;
                text(on_false)?;
                text(on_unknown)?;
                admit_condition(condition, 1, &mut budget)?;
            }
            Node::Terminal { action, .. } => {
                admit_action(action)?;
                if let Action::Select { overrides, .. } = action {
                    budget.spend(overrides.len())?;
                }
            }
        }
    }
    Ok(())
}
fn strings(values: &[String], budget: &mut Budget) -> Result<(), Error> {
    if values.len() > MAX_ITEMS {
        return Err(Error::new("resource-limit", "set exceeds 256 items"));
    }
    budget.spend(values.len())?;
    let mut unique = BTreeSet::new();
    for value in values {
        text(value)?;
        if !unique.insert(value) {
            return Err(invalid("duplicate set identity"));
        }
    }
    Ok(())
}
fn value(value: &Value, budget: &mut Budget) -> Result<(), Error> {
    budget.spend(1)?;
    match value {
        Value::Number(n) if !n.is_finite() => Err(invalid("number must be finite")),
        Value::Text(s) => text(s),
        Value::TextSet(s) => strings(s, budget),
        _ => Ok(()),
    }
}
fn key<'a>(
    key: &FactKey,
    catalog: &BTreeMap<&str, &'a FactDefinition>,
) -> Result<&'a FactDefinition, Error> {
    text(&key.name)?;
    let definition = catalog
        .get(key.name.as_str())
        .ok_or_else(|| invalid(format!("unregistered fact: {}", key.name)))?;
    match (definition.scope, &key.subject) {
        (Scope::Entity, Some(subject)) => text(subject)?,
        (Scope::Global, None) => {}
        _ => return Err(invalid("fact subject does not match catalog scope")),
    }
    Ok(definition)
}
fn predicate(predicate: &Predicate, kind: ValueType, budget: &mut Budget) -> Result<(), Error> {
    budget.spend(1)?;
    let valid = match predicate {
        Predicate::Eq { value: v } | Predicate::Ne { value: v } => {
            value(v, budget)?;
            v.value_type() == kind
        }
        Predicate::Lt { value }
        | Predicate::Le { value }
        | Predicate::Gt { value }
        | Predicate::Ge { value } => value.is_finite() && kind == ValueType::Number,
        Predicate::Range { min, max } => {
            kind == ValueType::Number && min.is_finite() && max.is_finite() && min <= max
        }
        Predicate::In { values } => {
            if values.len() > MAX_ITEMS {
                return Err(Error::new(
                    "resource-limit",
                    "membership exceeds 256 values",
                ));
            }
            for (index, v) in values.iter().enumerate() {
                value(v, budget)?;
                if v.value_type() != kind {
                    return Err(invalid("membership value type mismatch"));
                }
                if values[..index].contains(v) {
                    return Err(invalid("duplicate membership value"));
                }
            }
            kind != ValueType::TextSet
        }
        Predicate::Contains { value } => {
            text(value)?;
            kind == ValueType::TextSet
        }
        Predicate::ContainsAll { values } | Predicate::ContainsAny { values } => {
            strings(values, budget)?;
            kind == ValueType::TextSet
        }
        Predicate::Count { min, max } => kind == ValueType::TextSet && min <= max,
        Predicate::Present {} => true,
    };
    if !valid {
        return Err(invalid(
            "invalid predicate operator, bounds or operand type",
        ));
    }
    Ok(())
}
fn condition(
    condition: &Condition,
    depth: usize,
    catalog: &BTreeMap<&str, &FactDefinition>,
    budget: &mut Budget,
) -> Result<(), Error> {
    budget.spend(1)?;
    if depth > MAX_DEPTH {
        return Err(Error::new("resource-limit", "condition nesting exceeds 32"));
    }
    match condition {
        Condition::All { conditions } | Condition::Any { conditions } => {
            if conditions.len() > MAX_ITEMS {
                return Err(Error::new(
                    "resource-limit",
                    "boolean list exceeds 256 conditions",
                ));
            }
            for child in conditions {
                self::condition(child, depth + 1, catalog, budget)?;
            }
        }
        Condition::Not { condition: child } => self::condition(child, depth + 1, catalog, budget)?,
        Condition::Fact {
            key: k,
            predicate: p,
        } => predicate(p, key(k, catalog)?.value_type, budget)?,
    }
    Ok(())
}
pub(crate) fn action(action: &Action) -> Result<(), Error> {
    admit_action(action)?;
    let Action::Select {
        profile_id,
        overrides,
    } = action
    else {
        return Ok(());
    };
    if let Some(id) = profile_id {
        text(id)?;
    }
    if profile_id.is_none() && overrides.is_empty() {
        return Err(invalid(
            "select needs a profile ID or scoped overrides; use no-change otherwise",
        ));
    }
    if overrides.len() > MAX_ITEMS {
        return Err(Error::new("resource-limit", "too many overrides"));
    }
    let mut identities = BTreeSet::new();
    for change in overrides {
        let (identity, mute, gain) = match change {
            Override::Send {
                source,
                mix,
                mute,
                gain,
            } => {
                text(source)?;
                text(mix)?;
                (("send", source.as_str(), mix.as_str()), mute, gain)
            }
            Override::Output { output, mute, gain } => {
                text(output)?;
                (("output", output.as_str(), ""), mute, gain)
            }
            Override::Mix { mix, mute, gain } => {
                text(mix)?;
                (("mix", mix.as_str(), ""), mute, gain)
            }
        };
        if !identities.insert(identity) {
            return Err(invalid("duplicate override identity"));
        }
        // Automation attenuation never unmutes or increases an existing route's level.
        if mute == &Some(false)
            || (mute.is_none() && gain.is_none())
            || gain.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err(invalid(
                "override needs mute:true and/or finite gain in [0,1]; unmute is not supported",
            ));
        }
    }
    Ok(())
}

/// Validate the entire graph, including unreachable nodes, before inspecting a condition.
pub fn validate(request: &Request) -> Result<(), Error> {
    validate_parts(&request.graph, &request.catalog, &request.snapshot)
}

/// Borrowed bounded policy validation, without copying an unadmitted graph.
/// Used by durable settings; no observation is fabricated or evaluated.
pub fn validate_policy(graph: &Graph, catalog: &[FactDefinition]) -> Result<(), Error> {
    let snapshot = Snapshot {
        schema_version: 1,
        revision: 0,
        epoch: "policy-validation".into(),
        facts: vec![],
    };
    validate_parts(graph, catalog, &snapshot)
}
fn validate_parts(
    graph: &Graph,
    catalog: &[FactDefinition],
    snapshot: &Snapshot,
) -> Result<(), Error> {
    admit(graph, catalog, snapshot)?;
    if graph.schema_version != 1 || snapshot.schema_version != 1 {
        return Err(Error::new(
            "unsupported-version",
            "expected graph and observation schemaVersion 1",
        ));
    }
    let mut budget = Budget(0);
    text(&graph.id)?;
    text(&graph.name)?;
    text(&graph.root)?;
    text(&snapshot.epoch)?;
    if graph.nodes.is_empty()
        || graph.nodes.len() > MAX_NODES
        || catalog.len() > MAX_FACTS
        || snapshot.facts.len() > MAX_FACTS
    {
        return Err(Error::new(
            "resource-limit",
            "graph needs 1..256 nodes; catalog and observations at most 512 facts",
        ));
    }
    let standard = standard_catalog();
    let mut definitions = BTreeMap::new();
    for definition in catalog {
        budget.spend(1)?;
        text(&definition.name)?;
        if definitions
            .insert(definition.name.as_str(), definition)
            .is_some()
        {
            return Err(invalid("duplicate catalog identity"));
        }
        if let Some(reserved) = standard.iter().find(|d| d.name == definition.name) {
            if reserved.value_type != definition.value_type
                || reserved.scope != definition.scope
                || reserved.evidence_policy != definition.evidence_policy
            {
                return Err(invalid(
                    "reserved catalog fact type, scope or evidence policy changed",
                ));
            }
        }
    }
    let mut facts = BTreeSet::new();
    for fact in &snapshot.facts {
        budget.spend(1)?;
        let definition = key(&fact.key, &definitions)?;
        if !facts.insert(&fact.key) {
            return Err(invalid(
                "duplicate observation identity; consolidate ambiguity explicitly",
            ));
        }
        text(&fact.provider)?;
        if let Some(reason) = &fact.reason {
            text(reason)?;
        }
        match (fact.status, &fact.value) {
            (FactStatus::Known, Some(v)) => {
                value(v, &mut budget)?;
                if v.value_type() != definition.value_type {
                    return Err(invalid("observed value type mismatch"));
                }
            }
            (FactStatus::Known, None) => return Err(invalid("known observation needs a value")),
            (_, Some(_)) => return Err(invalid("non-known observation must not supply a value")),
            (_, None) => {}
        }
    }
    let mut nodes = BTreeMap::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        budget.spend(1)?;
        text(node.id())?;
        if nodes.insert(node.id(), index).is_some() {
            return Err(invalid("duplicate node identity"));
        }
        match node {
            Node::Decision { condition: c, .. } => condition(c, 1, &definitions, &mut budget)?,
            Node::Terminal { action: a, .. } => {
                action(a)?;
                if let Action::Select { overrides, .. } = a {
                    budget.spend(overrides.len())?;
                }
            }
        }
    }
    if !nodes.contains_key(graph.root.as_str()) {
        return Err(invalid("missing explicit root node"));
    }
    let mut edges = vec![Vec::new(); nodes.len()];
    let mut incoming = vec![0; nodes.len()];
    for (index, node) in graph.nodes.iter().enumerate() {
        if let Node::Decision {
            on_true,
            on_false,
            on_unknown,
            ..
        } = node
        {
            for next in [on_true, on_false, on_unknown] {
                let target = *nodes
                    .get(next.as_str())
                    .ok_or_else(|| invalid(format!("missing branch reference: {next}")))?;
                edges[index].push(target);
                incoming[target] += 1;
            }
        }
    }
    // Topological validation avoids recursive graph walks and repeated DAG expansion.
    let mut queue: VecDeque<_> = incoming
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (*n == 0).then_some(i))
        .collect();
    let mut depths = vec![1; nodes.len()];
    let mut visited = 0;
    while let Some(index) = queue.pop_front() {
        visited += 1;
        if depths[index] > MAX_DEPTH {
            return Err(Error::new("resource-limit", "graph path exceeds 32 nodes"));
        }
        for &next in &edges[index] {
            depths[next] = depths[next].max(depths[index] + 1);
            incoming[next] -= 1;
            if incoming[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    if visited != nodes.len() {
        return Err(invalid("decision graph contains a cycle"));
    }
    Ok(())
}
