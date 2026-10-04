//! Actual provider encoded vectors, excluding floating sample evidence containers.
use serde_json::Value;
use shr_desk::audio::{self, Context, Request};
#[test]
fn actual_preview_renew_cancel_release_expiry_and_command_codec() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/gp03/preview1/preview-flow.json")).unwrap();
    let mut previews = 0;
    let mut renews = 0;
    let mut finals = 0;
    for flow in ["cancel_and_commit", "expiry_with_live_lease"] {
        for node in corpus[flow].as_array().unwrap() {
            if let Some(snapshot) = node.get("snapshot") {
                audio::decode_snapshot(&serde_json::to_vec(snapshot).unwrap()).unwrap();
            }
            if let Some(request) = node.get("request") {
                let context:Context=serde_json::from_value(serde_json::json!({"show_id":request["show_id"],"module":request["module"],"epoch":request["epoch"],"writer":request["writer"],"lease":request["lease"],"request_id":request["request_id"],"expected_revision":request["expected_revision"]})).unwrap();
                let decoded = Request {
                    context,
                    kind: request["kind"].as_str().unwrap().into(),
                    body: request["body"].clone(),
                };
                assert_eq!(
                    serde_json::from_slice::<Value>(&decoded.encode().unwrap()).unwrap(),
                    *request
                );
                if request["kind"] == "renew" {
                    renews += 1;
                    assert_eq!(node["response"]["outcome"]["body"]["scope"], "foh");
                    assert!(node["response"]["outcome"]["body"]["granted_lease"].is_null());
                }
            }
            for response in node.get("response").into_iter().chain(
                node.get("completions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            ) {
                let reply = audio::decode_reply(&serde_json::to_vec(response).unwrap())
                    .unwrap_or_else(|e| panic!("{flow}/{}: {e}", node["label"]));
                if reply
                    .outcome
                    .as_ref()
                    .is_some_and(|o| o.body.preview.is_some())
                {
                    previews += 1;
                }
                if reply.state == "final" {
                    finals += 1;
                }
            }
        }
    }
    assert_eq!(renews, 2);
    assert_eq!(previews, 3);
    assert!(finals >= 12);
}
