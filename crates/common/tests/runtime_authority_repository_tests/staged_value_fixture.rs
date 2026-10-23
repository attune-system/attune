use serde_json::{json, Value};

pub(super) struct StagedValueDefinition {
    pub action_input: Value,
    pub action_output: Value,
    pub workflow_input: Value,
    pub workflow_output: Value,
    pub graph: Value,
}

impl StagedValueDefinition {
    pub fn assert_coherent_and_compiles(&self) {
        use attune_common::value_schema::{compile_inline, SchemaContext};

        let graph_input = &self.graph["parameters"];
        let graph_output = &self.graph["output"];
        assert_eq!(self.action_input, self.workflow_input);
        assert_eq!(&self.action_input, graph_input);
        assert_eq!(self.action_output, self.workflow_output);
        assert_eq!(&self.action_output, graph_output);
        for (name, schema) in [
            ("action.param_schema", &self.action_input),
            ("workflow.param_schema", &self.workflow_input),
            ("graph.parameters", graph_input),
            ("action.out_schema", &self.action_output),
            ("workflow.out_schema", &self.workflow_output),
            ("graph.output", graph_output),
        ] {
            compile_inline(name, SchemaContext::FlatDocument, schema).unwrap();
        }
        assert_eq!(graph_output["token"]["type"], "string");
        assert_eq!(graph_output["token"]["default"], "{{ second }}");
    }
}

pub(super) fn definition(leaf: &str, with_condition: bool) -> StagedValueDefinition {
    let input = json!({"supplied":{"type":"string"},"gate":{"type":"string"}});
    let output = json!({"token":{"type":"string","default":"{{ second }}"}});
    let tasks = if with_condition {
        json!([{"name":"publish","action":leaf,"publish":{"first":"{{ keystore[parameters.supplied] }}"},"next":[{"when":"{{ first != null and keystore[parameters.gate] }}","publish":{"second":"{{ first }}"}},{"when":"{{ first == null }}","publish":{"denied":"unreachable"}}]}])
    } else {
        json!([{"name":"publish","action":leaf,"publish":{"first":"{{ parameters.supplied }}","second":"{{ first }}"}}])
    };
    StagedValueDefinition {
        action_input: input.clone(),
        action_output: output.clone(),
        workflow_input: input.clone(),
        workflow_output: output.clone(),
        graph: json!({"parameters":input,"output":output,"tasks":tasks}),
    }
}

#[test]
fn staged_value_action_workflow_and_graph_schemas_compile_and_match_exactly() {
    for with_condition in [false, true] {
        definition("fixture.staged_leaf", with_condition).assert_coherent_and_compiles();
    }
}
