use super::query_arguments::into_model_query_arguments;
use crate::{
    protobuf::{mutaction::*, prelude::*, InputValidation},
    BridgeError, BridgeResult, ExternalInterface,
};
use connector::*;
use prisma_common::{config::WithMigrations, config::*};
use prisma_models::prelude::*;
use prost::Message;
use sqlite_connector::Sqlite;
use std::sync::Arc;

pub struct ProtoBufInterface {
    data_resolver: Arc<DataResolver + Send + Sync + 'static>,
    database_mutaction_executor: Arc<DatabaseMutactionExecutor + Send + Sync + 'static>,
}

impl ProtoBufInterface {
    pub fn new(config: &PrismaConfig) -> ProtoBufInterface {
        let connector = match config.databases.get("default") {
            Some(PrismaDatabase::Explicit(ref config)) if config.connector == "sqlite-native" => {
                // FIXME: figure out the right way to do it
                // we are passing is_active as test_mode parameter
                // this requires us to put `active: true` in all sqlite-native configs used in tests
                let sqlite = Sqlite::new(config.limit(), config.is_active().unwrap()).unwrap();

                Arc::new(sqlite)
            }
            _ => panic!("Database connector is not supported, use sqlite with a file for now!"),
        };

        ProtoBufInterface {
            data_resolver: connector.clone(),
            database_mutaction_executor: connector,
        }
    }

    fn protobuf_result<F>(f: F) -> Vec<u8>
    where
        F: FnOnce() -> BridgeResult<Vec<u8>>,
    {
        f().unwrap_or_else(|error| match error {
            BridgeError::ConnectorError(ConnectorError::NodeDoesNotExist) => {
                let response = prisma::RpcResponse::empty();
                let mut response_payload = Vec::new();

                response.encode(&mut response_payload).unwrap();
                response_payload
            }
            _ => {
                dbg!(&error);

                let error_response = prisma::RpcResponse::error(error);

                let mut payload = Vec::new();
                error_response.encode(&mut payload).unwrap();
                payload
            }
        })
    }
}

impl InputValidation for GetNodeByWhereInput {
    fn validate(&self) -> BridgeResult<()> {
        Ok(())
    }
}

impl ExternalInterface for ProtoBufInterface {
    fn get_node_by_where(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetNodeByWhereInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let selected_fields = input.selected_fields.into_selected_fields(model.clone(), None);

            let value: PrismaValue = input.value.into();
            let field = model.fields().find_from_scalar(&input.field_name)?;
            let node_selector = NodeSelector { field, value };

            let query_result = self.data_resolver.get_node_by_where(node_selector, &selected_fields)?;

            let (nodes, fields) = match query_result {
                Some(node) => (vec![node.node.into()], node.field_names),
                _ => (Vec::new(), Vec::new()),
            };

            let response = RpcResponse::ok(prisma::NodesResult { nodes, fields });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_nodes(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetNodesInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let selected_fields = input.selected_fields.into_selected_fields(model.clone(), None);
            let query_arguments = into_model_query_arguments(model.clone(), input.query_arguments);

            let query_result = self.data_resolver.get_nodes(model, query_arguments, &selected_fields)?;
            let (nodes, fields) = (query_result.nodes, query_result.field_names);
            let proto_nodes = nodes.into_iter().map(|n| n.into()).collect();

            let response = RpcResponse::ok(prisma::NodesResult {
                nodes: proto_nodes,
                fields: fields,
            });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_related_nodes(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetRelatedNodesInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;

            let project: ProjectRef = project_template.into();
            let model = project.schema().find_model(&input.model_name)?;

            let from_field = model.fields().find_from_relation_fields(&input.from_field)?;
            let from_node_ids: Vec<GraphqlId> = input.from_node_ids.into_iter().map(GraphqlId::from).collect();
            let related_model = from_field.related_model();

            let selected_fields = input
                .selected_fields
                .into_selected_fields(Arc::clone(&related_model), Some(from_field.clone()));

            let query_result = self.data_resolver.get_related_nodes(
                from_field,
                &from_node_ids,
                into_model_query_arguments(Arc::clone(&related_model), input.query_arguments),
                &selected_fields,
            )?;

            let (nodes, fields) = (query_result.nodes, query_result.field_names);
            let proto_nodes = nodes.into_iter().map(|n| n.into()).collect();

            let response = RpcResponse::ok(prisma::NodesResult {
                nodes: proto_nodes,
                fields: fields,
            });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_scalar_list_values_by_node_ids(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetScalarListValuesByNodeIds::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let list_field = model.fields().find_from_scalar(&input.list_field)?;

            let node_ids: Vec<GraphqlId> = input.node_ids.into_iter().map(GraphqlId::from).collect();

            let query_result = self
                .data_resolver
                .get_scalar_list_values_by_node_ids(list_field, node_ids)?;

            let proto_values = query_result
                .into_iter()
                .map(|vals| prisma::ScalarListValues {
                    node_id: vals.node_id.into(),
                    values: vals.values.into_iter().map(|n| n.into()).collect(),
                })
                .collect();

            let response = RpcResponse::ok(prisma::ScalarListValuesResult { values: proto_values });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn count_by_model(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = CountByModelInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();
            let model = project.schema().find_model(&input.model_name)?;

            let query_arguments = into_model_query_arguments(model.clone(), input.query_arguments);
            let count = self.data_resolver.count_by_model(model, query_arguments)?;

            let response = RpcResponse::ok(count);

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn count_by_table(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = CountByTableInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let count = match project.schema().find_model(&input.model_name) {
                Ok(model) => self
                    .data_resolver
                    .count_by_table(project.schema().db_name.as_ref(), model.db_name()),
                Err(_) => self
                    .data_resolver
                    .count_by_table(project.schema().db_name.as_ref(), &input.model_name),
            }?;

            let response = RpcResponse::ok(count);

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn execute_raw(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = ExecuteRawInput::decode(payload)?;
            let json = self.database_mutaction_executor.execute_raw(input.query)?;
            let json_as_string = serde_json::to_string(&json)?;

            let response = RpcResponse::ok_raw(prisma::ExecuteRawResult { json: json_as_string });
            let mut response_payload = Vec::new();

            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn execute_mutaction(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = crate::protobuf::prisma::DatabaseMutaction::decode(payload)?;
            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let parent_id = input.parent_id.clone().map(GraphqlId::from);
            let mutaction = convert_mutaction(input, Arc::clone(&project));
            let db_name = project.schema().db_name.to_string();

            let mut results = match mutaction {
                DatabaseMutaction::TopLevel(tlm) => self.database_mutaction_executor.execute_toplevel(db_name, tlm)?,

                DatabaseMutaction::Nested(catcatmeow) => {
                    self.database_mutaction_executor
                        .execute_nested(db_name, catcatmeow, parent_id.unwrap())?
                }
            };

            let result = results.pop().expect("no mutaction results returned");

            let response = RpcResponse::ok_mutaction(convert_mutaction_result(result));
            let mut response_payload = Vec::new();

            response.encode(&mut response_payload).unwrap();
            Ok(response_payload)
        })
    }
}
