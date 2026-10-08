use composenest_application::{
    RequestContext,
    clone_plan::{CloneAnswer, CloneEdit},
    clone_session::{ConfirmCloneRequest, PrepareCloneRequest, UpdateCloneRequest},
    create_plan::PlanEdit,
    create_session::{
        ConfirmCreateRequest, CreatePlanRequest, PrepareCreateRequest, UpdateCreateRequest,
    },
    instance_actions::{ChangeInstanceRequest, InstanceActionRequest, RenameInstanceRequest},
    instance_content::{InstanceComposeRequest, InstanceSecretRequest},
    instance_edit::EditInstancePortsRequest,
    log_subscription::{LogSubscriptionRequest, SubscribeLogsRequest},
    query_service::OperationRequest,
    recovery_view::RecoveryRequest,
    settings::SaveSettingsRequest,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn rejects_injected_fields<T: DeserializeOwned>(valid: Value) {
    assert!(serde_json::from_value::<T>(valid.clone()).is_ok());
    for key in [
        "unknown",
        "scopeId",
        "targetId",
        "endpoint",
        "dockerConfig",
        "hostPath",
        "command",
        "compose",
    ] {
        let mut injected = valid.clone();
        injected[key] = json!("attacker-controlled");
        assert!(
            serde_json::from_value::<T>(injected).is_err(),
            "{} accepted {key}",
            std::any::type_name::<T>()
        );
    }
    if valid.get("context").is_some() {
        let mut injected = valid;
        injected["context"]["endpoint"] = json!("tcp://attacker:2375");
        assert!(serde_json::from_value::<T>(injected).is_err());
    }
}

#[test]
fn desktop_requests_reject_unknown_fields_and_backend_target_substitution() {
    let context = json!({"apiVersion": 1, "requestId": "security"});
    let instance = json!({"context": context, "instanceId": "instance"});
    let plan = json!({"context": context, "planId": "plan"});
    rejects_injected_fields::<RequestContext>(context.clone());
    rejects_injected_fields::<PrepareCreateRequest>(
        json!({"context":context,"templateRevisionId":"revision"}),
    );
    rejects_injected_fields::<PrepareCloneRequest>(json!({"context":context,"sourceId":"source"}));
    rejects_injected_fields::<CreatePlanRequest>(plan.clone());
    rejects_injected_fields::<InstanceActionRequest>(instance.clone());
    rejects_injected_fields::<RenameInstanceRequest>(
        json!({"context":context,"instanceId":"instance","expectedRevision":1,"name":"Name"}),
    );
    rejects_injected_fields::<ChangeInstanceRequest>(
        json!({"context":context,"instanceId":"instance","expectedRevision":1,"action":"start"}),
    );
    rejects_injected_fields::<EditInstancePortsRequest>(
        json!({"context":context,"instanceId":"instance","expectedRevision":1,"expectedSpecRevision":1,"ports":{}}),
    );
    rejects_injected_fields::<InstanceSecretRequest>(
        json!({"context":context,"instanceId":"instance","expectedSpecRevision":1,"slot":"password"}),
    );
    rejects_injected_fields::<InstanceComposeRequest>(
        json!({"context":context,"instanceId":"instance","expectedSpecRevision":1,"reveal":false}),
    );
    rejects_injected_fields::<SubscribeLogsRequest>(
        json!({"context":context,"instanceId":"instance","expectedSpecRevision":1}),
    );
    rejects_injected_fields::<LogSubscriptionRequest>(
        json!({"context":context,"subscriptionId":"logs"}),
    );
    rejects_injected_fields::<OperationRequest>(
        json!({"context":context,"operationId":"operation"}),
    );
    rejects_injected_fields::<RecoveryRequest>(
        json!({"context":context,"instanceId":"instance","operationId":"operation"}),
    );
    rejects_injected_fields::<SaveSettingsRequest>(
        json!({"context":context,"storageMethod":"bind"}),
    );
    let confirm = json!({"context":context,"planId":"plan","revision":1,"confirmedPorts":{},"acceptPlaintext":true});
    rejects_injected_fields::<ConfirmCreateRequest>(confirm.clone());
    let mut clone = confirm;
    clone["acceptConfigurationOnly"] = json!(true);
    rejects_injected_fields::<ConfirmCloneRequest>(clone);
    let edit = json!({"expectedRevision":1});
    rejects_injected_fields::<PlanEdit>(edit.clone());
    rejects_injected_fields::<CloneEdit>(edit.clone());
    let mut update = plan;
    update["edit"] = edit;
    rejects_injected_fields::<UpdateCreateRequest>(update.clone());
    rejects_injected_fields::<UpdateCloneRequest>(update.clone());
    update["edit"]["endpoint"] = json!("tcp://attacker:2375");
    assert!(serde_json::from_value::<UpdateCreateRequest>(update.clone()).is_err());
    assert!(serde_json::from_value::<UpdateCloneRequest>(update).is_err());
}

#[test]
fn clone_answers_reject_unknown_nested_fields() {
    for action in ["copy", "generate", "clear", "input"] {
        let mut answer = json!({"action":action});
        if action == "input" {
            answer["value"] = json!("explicit input");
        }
        rejects_injected_fields::<CloneAnswer>(answer);
    }
}
