//! Members named with friendly names ("Quantidade Total") are GraphQL fields
//! in lower camel case, translated back to the member in the query and to the
//! field in the response.

use cubegraphql::{execute_request, executor_fn, make_schema, CubeQueryRequest, MetaConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn meta() -> MetaConfig {
    MetaConfig::from_value(&json!({ "cubes": [{
        "name": "bossa",
        "measures": [
            { "name": "bossa.Quantidade Total", "type": "number", "isVisible": true },
            { "name": "bossa.count", "type": "number", "isVisible": true }
        ],
        "dimensions": [
            { "name": "bossa.Fabricante do Cabo", "type": "string", "isVisible": true },
            { "name": "bossa.Data de Fabricação", "type": "time", "isVisible": true }
        ]
    }]}))
    .unwrap()
}

#[tokio::test]
async fn friendly_names_round_trip() {
    let schema = make_schema(&meta()).unwrap();
    let sdl = schema.sdl();
    assert!(sdl.contains("quantidadeTotal"), "{sdl}");
    assert!(sdl.contains("fabricanteDoCabo"), "{sdl}");
    assert!(sdl.contains("dataDeFabricacao"), "{sdl}");

    let seen: Arc<Mutex<Option<Value>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let executor = executor_fn(move |req: CubeQueryRequest| {
        *sink.lock().unwrap() = Some(req.query.as_value().clone());
        async move {
            Ok(json!({
                "query": {},
                "annotation": {
                    "measures": { "bossa.Quantidade Total": { "type": "number" } },
                    "dimensions": { "bossa.Fabricante do Cabo": { "type": "string" } },
                    "segments": {},
                    "timeDimensions": { "bossa.Data de Fabricação.month": { "type": "time" } }
                },
                "data": [{
                    "bossa.Quantidade Total": 12,
                    "bossa.Fabricante do Cabo": "AFL",
                    "bossa.Data de Fabricação.month": "2025-03-01T00:00:00.000"
                }]
            }))
        }
    });
    let doc = r#"query {
        cube(where: { bossa: { fabricanteDoCabo: { equals: "AFL" } } }, orderBy: { bossa: { quantidadeTotal: desc } }) {
            bossa(orderBy: { fabricanteDoCabo: asc }, where: { OR: [{ fabricanteDoCabo: { equals: "OFS" } }] }) {
                quantidadeTotal fabricanteDoCabo dataDeFabricacao { month }
            }
        }
    }"#;
    let response = execute_request(&schema, async_graphql::Request::new(doc), executor).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let query = seen.lock().unwrap().clone().unwrap();
    assert_eq!(query["measures"], json!(["bossa.Quantidade Total"]));
    assert_eq!(query["dimensions"], json!(["bossa.Fabricante do Cabo"]));
    assert_eq!(query["timeDimensions"][0]["dimension"], "bossa.Data de Fabricação");
    let text = query.to_string();
    assert!(!text.contains("fabricanteDoCabo") && !text.contains("quantidadeTotal"), "{text}");
    assert!(text.contains(r#"["bossa.Quantidade Total","desc"]"#), "{text}");
    assert!(text.contains(r#""member":"bossa.Fabricante do Cabo""#), "{text}");

    let body = serde_json::to_value(&response).unwrap();
    let row = &body["data"]["cube"][0]["bossa"];
    assert_eq!(row["quantidadeTotal"], 12.0);
    assert_eq!(row["fabricanteDoCabo"], "AFL");
    assert!(row["dataDeFabricacao"]["month"].is_string(), "{body}");
}
