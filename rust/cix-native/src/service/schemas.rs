//! One registry defines request validation metadata and transport discovery.
use super::contracts::*;
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};

pub struct OperationSchema {
    pub name: &'static str,
    pub scope: &'static str,
    pub description_id: &'static str,
    pub input: Value,
    pub output: Value,
    pub read_only: bool,
    pub method: &'static str,
    pub path: &'static str,
}
pub fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).expect("JSON schema serialization")
}
pub fn operations() -> Vec<OperationSchema> {
    let empty = json!({"type":"object","additionalProperties":false});
    let id = json!({"type":"object","required":["id"],"properties":{"id":{"type":"string","format":"uuid"}},"additionalProperties":false});
    let page = json!({"type":"object","properties":{"cursor":{"type":"string"}},"additionalProperties":false});
    let mut with_cursor = id.clone();
    with_cursor["properties"]["cursor"] = json!({"type":"string"});
    vec![
        op(
            "cix_capabilities",
            "service:read",
            "service.tool.capabilities",
            empty,
            json!({"type":"object"}),
            true,
            "get",
            "/v1/capabilities",
        ),
        op(
            "cix_jobs_submit",
            "jobs:write",
            "service.tool.jobs_submit",
            schema::<TransformRequest>(),
            schema::<Job>(),
            false,
            "post",
            "/v1/jobs",
        ),
        op(
            "cix_jobs_get",
            "jobs:read",
            "service.tool.jobs_get",
            id.clone(),
            schema::<Job>(),
            true,
            "get",
            "/v1/jobs/{id}",
        ),
        op(
            "cix_jobs_list",
            "jobs:read",
            "service.tool.jobs_list",
            page.clone(),
            schema::<Page<Job>>(),
            true,
            "get",
            "/v1/jobs",
        ),
        op(
            "cix_jobs_cancel",
            "jobs:write",
            "service.tool.jobs_cancel",
            id.clone(),
            schema::<Job>(),
            false,
            "post",
            "/v1/jobs/{id}/cancel",
        ),
        op(
            "cix_jobs_events",
            "jobs:read",
            "service.tool.jobs_events",
            with_cursor,
            schema::<Page<Event>>(),
            true,
            "get",
            "/v1/jobs/{id}/events",
        ),
        op(
            "cix_artifacts_get",
            "artifacts:read",
            "service.tool.artifacts_get",
            id.clone(),
            schema::<Artifact>(),
            true,
            "get",
            "/v1/artifacts/{id}",
        ),
        op(
            "cix_artifacts_upload",
            "artifacts:write",
            "service.tool.artifacts_upload",
            json!({"type":"object","properties":{"base64":{"type":"string","maxLength":87384}},"required":["base64"],"additionalProperties":false}),
            schema::<Artifact>(),
            false,
            "post",
            "/v1/artifacts/inline",
        ),
        op(
            "cix_collections_create",
            "collections:write",
            "service.tool.collections_create",
            schema::<CreateCollection>(),
            schema::<Collection>(),
            false,
            "post",
            "/v1/collections",
        ),
        op(
            "cix_collections_get",
            "collections:read",
            "service.tool.collections_get",
            id,
            schema::<Collection>(),
            true,
            "get",
            "/v1/collections/{id}",
        ),
        op(
            "cix_collections_list",
            "collections:read",
            "service.tool.collections_list",
            page,
            schema::<Page<Collection>>(),
            true,
            "get",
            "/v1/collections",
        ),
    ]
}
fn op(
    name: &'static str,
    scope: &'static str,
    description_id: &'static str,
    input: Value,
    output: Value,
    read_only: bool,
    method: &'static str,
    path: &'static str,
) -> OperationSchema {
    OperationSchema {
        name,
        scope,
        description_id,
        input,
        output,
        read_only,
        method,
        path,
    }
}
pub fn openapi(compatibility: bool) -> Value {
    let mut document = json!({"openapi":if compatibility {"3.1.2"} else {"3.2.1"},"info":{"title":"CIX service API","version":"1.0.0"},"paths":{},"components":{"securitySchemes":{"bearer":{"type":"http","scheme":"bearer"}}},"security":[{"bearer":[]} ]});
    document["x-cix-authorization"] = json!({"identity":["provider","tenant","subject"],"namespace":"service, jobs, artifacts or collections", "collection":"stored collection identifier when applicable", "profile":"actual fast/default/best when applicable", "root":"logical object store: objects", "codec":"unknown for automatic selection; a CIX archive format is not an exact selected codec", "unknown_selector":"does not match a constrained allow rule", "metadata_schema":2, "list_cursors":"bounded raw scan; a filtered page may be empty and still have a next cursor", "metadata_budget":"configured max_json_bytes output, actual JSON input, input+output+4096 memory admission, zero temporary, one worker, 1000ms admission; not an RSS or database deadline guarantee"});
    for o in operations() {
        let mut item = json!({"operationId":o.name,"x-cix-description-id":o.description_id,"x-cix-scope":o.scope,"responses":{"200":{"description":"Successful operation","content":{"application/json":{"schema":o.output}}}},"parameters":[]});
        if o.path.contains("{id}") {
            item["parameters"].as_array_mut().unwrap().push(json!({"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        if o.method == "post" && !o.path.ends_with("/cancel") {
            item["requestBody"] =
                json!({"required":true,"content":{"application/json":{"schema":o.input}}});
        }
        if o.method == "get" && o.input["properties"].get("cursor").is_some() {
            item["parameters"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"cursor","in":"query","schema":{"type":"string"}}));
        }
        for code in [
            "400", "401", "403", "404", "409", "413", "422", "429", "503",
        ] {
            item["responses"][code] = json!({"description":"Typed service error","content":{"application/problem+json":{"schema":schema::<super::error::Problem>()}}});
        }
        document["paths"][o.path][o.method] = item;
    }
    document
}
