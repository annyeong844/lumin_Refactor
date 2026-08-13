use oxc_ast::ast::{Class, ClassElement, Decorator};
use oxc_span::GetSpan;

#[derive(Debug)]
pub(super) struct ClassEvaluationPhases {
    pre_static_evaluations: Vec<(u32, u32)>,
    static_executions: Vec<(u32, u32)>,
}

impl ClassEvaluationPhases {
    pub(super) fn from_class(class: &Class<'_>) -> Option<Self> {
        let mut pre_static_evaluations = Vec::new();
        let mut static_executions = Vec::new();

        push_decorator_spans(&mut pre_static_evaluations, &class.decorators);
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    push_decorator_spans(&mut pre_static_evaluations, &method.decorators);
                    if method.computed {
                        push_span(&mut pre_static_evaluations, method.key.span());
                    }
                }
                ClassElement::PropertyDefinition(property) => {
                    push_decorator_spans(&mut pre_static_evaluations, &property.decorators);
                    if property.computed {
                        push_span(&mut pre_static_evaluations, property.key.span());
                    }
                    if property.r#static
                        && let Some(value) = &property.value
                    {
                        push_span(&mut static_executions, value.span());
                    }
                }
                ClassElement::AccessorProperty(property) => {
                    push_decorator_spans(&mut pre_static_evaluations, &property.decorators);
                    if property.computed {
                        push_span(&mut pre_static_evaluations, property.key.span());
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

        (!pre_static_evaluations.is_empty() && !static_executions.is_empty()).then_some(Self {
            pre_static_evaluations,
            static_executions,
        })
    }

    pub(super) fn pre_static_evaluation_contains(&self, position: u32) -> bool {
        contains(&self.pre_static_evaluations, position)
    }

    pub(super) fn static_execution_contains(&self, position: u32) -> bool {
        contains(&self.static_executions, position)
    }
}

fn push_decorator_spans(ranges: &mut Vec<(u32, u32)>, decorators: &[Decorator<'_>]) {
    for decorator in decorators {
        push_span(ranges, decorator.expression.span());
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
