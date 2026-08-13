use oxc_ast::ast::{Class, ClassElement};
use oxc_span::GetSpan;

#[derive(Debug)]
pub(super) struct ClassEvaluationPhases {
    computed_keys: Vec<(u32, u32)>,
    static_executions: Vec<(u32, u32)>,
}

impl ClassEvaluationPhases {
    pub(super) fn from_class(class: &Class<'_>) -> Option<Self> {
        let mut computed_keys = Vec::new();
        let mut static_executions = Vec::new();

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    if method.computed {
                        push_span(&mut computed_keys, method.key.span());
                    }
                }
                ClassElement::PropertyDefinition(property) => {
                    if property.computed {
                        push_span(&mut computed_keys, property.key.span());
                    }
                    if property.r#static
                        && let Some(value) = &property.value
                    {
                        push_span(&mut static_executions, value.span());
                    }
                }
                ClassElement::AccessorProperty(property) => {
                    if property.computed {
                        push_span(&mut computed_keys, property.key.span());
                    }
                    if property.r#static
                        && let Some(value) = &property.value
                    {
                        push_span(&mut static_executions, value.span());
                    }
                }
                ClassElement::StaticBlock(block) => {
                    push_span(&mut static_executions, block.span);
                }
                ClassElement::TSIndexSignature(_) => {}
            }
        }

        (!computed_keys.is_empty() && !static_executions.is_empty()).then_some(Self {
            computed_keys,
            static_executions,
        })
    }

    pub(super) fn computed_key_contains(&self, position: u32) -> bool {
        contains(&self.computed_keys, position)
    }

    pub(super) fn static_execution_contains(&self, position: u32) -> bool {
        contains(&self.static_executions, position)
    }
}

fn push_span(ranges: &mut Vec<(u32, u32)>, span: oxc_span::Span) {
    ranges.push((span.start, span.end));
}

fn contains(ranges: &[(u32, u32)], position: u32) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= position && position <= *end)
}
