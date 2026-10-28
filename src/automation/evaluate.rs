use super::*;
use std::collections::{BTreeMap, BTreeSet};

struct Evaluator<'a> {
    request: &'a Request,
    catalog: BTreeMap<&'a str, &'a FactDefinition>,
    facts: BTreeMap<&'a FactKey, &'a Fact>,
    observations: Vec<Fact>,
    observation_indices: BTreeMap<&'a FactKey, usize>,
    trace: Vec<Trace>,
    trace_bytes: usize,
    truncated: bool,
}
impl Evaluator<'_> {
    fn record(&mut self, event: Trace) {
        // Include array punctuation in the byte budget. Once full, preserve a stable prefix.
        if self.truncated {
            return;
        }
        let bytes = serde_json::to_vec(&event)
            .expect("validated finite trace data")
            .len()
            + 1;
        if self.trace.len() < MAX_TRACE && self.trace_bytes + bytes <= MAX_TRACE_BYTES {
            self.trace_bytes += bytes;
            self.trace.push(event);
        } else {
            self.truncated = true;
        }
    }
    fn fact(&mut self, node: &str, key: &FactKey, predicate: &Predicate) -> Truth {
        let observation = self.facts.get(key).copied();
        // Catalog existence, types and identities have already been validated.
        let definition = self.catalog[key.name.as_str()];
        let age_ms = observation
            .and_then(|f| f.observed_at_ms)
            .and_then(|at| self.request.now_ms.checked_sub(at));
        let fresh = observation
            .zip(age_ms)
            .is_some_and(|(f, age)| age <= f.valid_for_ms.min(definition.max_age_ms));
        let unknown_reason = match observation {
            None => Some("missing-observation"),
            Some(f) => match f.status {
                FactStatus::Unknown => Some("provider-unknown"),
                FactStatus::Unsupported => Some("provider-unsupported"),
                FactStatus::Error => Some("provider-error"),
                FactStatus::Ambiguous => Some("provider-ambiguous"),
                FactStatus::Known if f.observed_at_ms.is_none() => Some("missing-observed-time"),
                FactStatus::Known if age_ms.is_none() => {
                    Some("observed-time-in-future-or-wrong-epoch")
                }
                FactStatus::Known if !fresh => Some("stale-observation"),
                FactStatus::Known if f.completeness != Completeness::Complete => {
                    Some("incomplete-observation")
                }
                FactStatus::Known
                    if definition.evidence_policy == EvidencePolicy::UserDeclaration
                        && f.evidence != Evidence::UserDeclared =>
                {
                    Some("explicit-user-declaration-required")
                }
                FactStatus::Known => None,
            },
        };
        let result = if unknown_reason.is_some() {
            Truth::Unknown
        } else {
            let value = observation
                .and_then(|f| f.value.as_ref())
                .expect("validated known observation");
            if compare(value, predicate) {
                Truth::True
            } else {
                Truth::False
            }
        };
        // Evidence appears once, with trace references. Bound serialized expansion BEFORE cloning.
        let mut observation_index =
            observation.and_then(|f| self.observation_indices.get(&f.key).copied());
        if !self.truncated && observation_index.is_none() {
            if let Some(f) = observation {
                let bytes = serde_json::to_vec(f)
                    .expect("validated finite fact data")
                    .len()
                    + 1;
                if self.trace_bytes + bytes <= MAX_TRACE_BYTES {
                    observation_index = Some(self.observations.len());
                    self.observation_indices
                        .insert(&f.key, self.observations.len());
                    self.observations.push(f.clone());
                    self.trace_bytes += bytes;
                } else {
                    self.truncated = true;
                }
            }
        }
        if !self.truncated {
            self.record(Trace::Fact {
                node: node.into(),
                detail: FactTrace {
                    key: key.clone(),
                    predicate: predicate.clone(),
                    result,
                    observation_index,
                    age_ms,
                    fresh,
                    unknown_reason: unknown_reason.map(str::to_owned),
                },
            });
        }
        result
    }
    fn condition(&mut self, node: &str, condition: &Condition) -> Truth {
        match condition {
            Condition::Fact { key, predicate } => self.fact(node, key, predicate),
            Condition::Not { condition } => match self.condition(node, condition) {
                Truth::True => Truth::False,
                Truth::False => Truth::True,
                Truth::Unknown => Truth::Unknown,
            },
            Condition::All { conditions } | Condition::Any { conditions } => {
                let all = matches!(condition, Condition::All { .. });
                let mut unknown = false;
                let mut decisive = false;
                // Evaluate every child in document order, so explanation doesn't depend on short-circuiting.
                for child in conditions {
                    match self.condition(node, child) {
                        Truth::False if all => decisive = true,
                        Truth::True if !all => decisive = true,
                        Truth::Unknown => unknown = true,
                        _ => {}
                    }
                }
                if decisive {
                    if all {
                        Truth::False
                    } else {
                        Truth::True
                    }
                } else if unknown {
                    Truth::Unknown
                } else if all {
                    Truth::True
                } else {
                    Truth::False
                }
            }
        }
    }
}
fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::TextSet(a), Value::TextSet(b)) => {
            a.iter().collect::<BTreeSet<_>>() == b.iter().collect()
        }
        _ => a == b,
    }
}
fn compare(observed: &Value, predicate: &Predicate) -> bool {
    match predicate {
        Predicate::Eq { value } => equal(observed, value),
        Predicate::Ne { value } => !equal(observed, value),
        Predicate::In { values } => values.iter().any(|v| equal(observed, v)),
        Predicate::Present {} => match observed {
            Value::TextSet(v) => !v.is_empty(),
            _ => true,
        },
        _ => match observed {
            Value::Number(n) => match predicate {
                Predicate::Lt { value } => n < value,
                Predicate::Le { value } => n <= value,
                Predicate::Gt { value } => n > value,
                Predicate::Ge { value } => n >= value,
                Predicate::Range { min, max } => min <= n && n <= max,
                _ => unreachable!("validated numeric predicate"),
            },
            Value::TextSet(set) => match predicate {
                Predicate::Contains { value } => set.contains(value),
                Predicate::ContainsAll { values } => values.iter().all(|v| set.contains(v)),
                Predicate::ContainsAny { values } => values.iter().any(|v| set.contains(v)),
                Predicate::Count { min, max } => {
                    (*min as usize..=*max as usize).contains(&set.len())
                }
                _ => unreachable!("validated set predicate"),
            },
            _ => unreachable!("validated predicate type"),
        },
    }
}

/// Evaluate one deterministic path in a validated DAG; performs no I/O or clock reads.
/// Unsupported/missing information always follows onUnknown, never onFalse implicitly.
pub fn evaluate(request: &Request) -> Result<Evaluation, Error> {
    validate(request)?;
    let token = StateToken {
        graph_id: request.graph.id.clone(),
        graph_revision: request.graph.revision,
        snapshot_revision: request.snapshot.revision,
        epoch: request.snapshot.epoch.clone(),
        evaluated_at_ms: request.now_ms,
        context: request.context.clone(),
    };
    if request.manual_override {
        return Ok(Evaluation {
            schema_version: 1,
            token,
            terminal: None,
            action: Action::NoChange {},
            observations: vec![],
            trace: vec![Trace::ManualOverride],
            trace_truncated: false,
            audio_applied: false,
        });
    }
    let mut evaluator = Evaluator {
        request,
        catalog: request
            .catalog
            .iter()
            .map(|d| (d.name.as_str(), d))
            .collect(),
        facts: request.snapshot.facts.iter().map(|f| (&f.key, f)).collect(),
        observations: vec![],
        observation_indices: BTreeMap::new(),
        trace: vec![],
        trace_bytes: 4,
        truncated: false,
    };
    let nodes: BTreeMap<_, _> = request.graph.nodes.iter().map(|n| (n.id(), n)).collect();
    let mut current = request.graph.root.as_str();
    loop {
        match nodes[current] {
            Node::Decision {
                id,
                condition,
                on_true,
                on_false,
                on_unknown,
            } => {
                let result = evaluator.condition(id, condition);
                let next = match result {
                    Truth::True => on_true,
                    Truth::False => on_false,
                    Truth::Unknown => on_unknown,
                };
                evaluator.record(Trace::Decision {
                    node: id.clone(),
                    result,
                    next: next.clone(),
                });
                current = next;
            }
            Node::Terminal { id, action } => {
                evaluator.record(Trace::Terminal { node: id.clone() });
                return Ok(Evaluation {
                    schema_version: 1,
                    token,
                    terminal: Some(id.clone()),
                    action: action.clone(),
                    observations: evaluator.observations,
                    trace: evaluator.trace,
                    trace_truncated: evaluator.truncated,
                    audio_applied: false,
                });
            }
        }
    }
}
