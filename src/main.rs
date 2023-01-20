use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose, Engine as _};
use juniper::http::graphiql::graphiql_source;
use juniper::{
    graphql_object, DefaultScalarValue, EmptyMutation, EmptySubscription, ExecutionError,
    FieldResult, GraphQLError, GraphQLObject, GraphQLScalar, InputValue, RootNode, ScalarValue,
    Value,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::{net::SocketAddr, str::Utf8Error};

#[derive(Clone)]
struct Context;

impl Context {
    fn new() -> Self {
        Context {}
    }
}
impl juniper::Context for Context {}

#[derive(GraphQLScalar, Debug)]
#[graphql(name = "ID", to_output_with = to_id_output, from_input_with = ViewerID::from_input, transparent)]
pub struct ViewerID(i32);

impl ViewerID {
    fn from_input<S>(input: &InputValue<S>) -> Result<ViewerID, String>
    where
        S: ScalarValue,
    {
        input
            .as_string_value()
            .ok_or_else(|| format!("Expected `ViewerID` found: {input}"))
            .and_then(|str| {
                base64_decode(str).or_else(|_| {
                    Err(format!(
                        "Couldn't decode `ViewerID` input as base64 found: {input}"
                    ))
                })
            })
            .and_then(|str| {
                str.strip_prefix("Viewer:")
                    .ok_or_else(|| {
                        format!("Invalid `ViewerID` expected `Viewer:` prefix, found: {str}")
                    })
                    .map(|s| s.to_owned())
            })
            .and_then(|str| {
                str.parse::<i32>().or_else(|_| {
                    Err(format!(
                        "Expected `ViewerID` suffix to be an Int, found: {str}"
                    ))
                })
            })
            .map(|v| ViewerID(v))
    }
}

fn to_id_output<S: ScalarValue>(v: &ViewerID) -> Value<S> {
    Value::from(base64_encode(format!("Viewer:{}", v.0)))
}

#[derive(GraphQLObject)]
#[graphql(description = "The main Viewer object that represents context of a `caller`.")]
struct Viewer {
    id: ViewerID,
}

struct Query;

#[graphql_object(context = Context)]
impl Query {
    fn viewer(context: &Context) -> FieldResult<Viewer> {
        Ok(Viewer { id: ViewerID(1) })
    }
    fn node(context: &Context, id: ViewerID) -> FieldResult<Option<Viewer>> {
        Ok(Some(Viewer { id }))
    }
}

fn base64_decode(str: &str) -> Result<String, Utf8Error> {
    let raw_vec = general_purpose::STANDARD.decode(str).unwrap();
    let output = std::str::from_utf8(&raw_vec).unwrap();
    Ok(String::from(output))
}

fn base64_encode<T>(input: T) -> String
where
    T: AsRef<[u8]>,
{
    general_purpose::STANDARD.encode(input)
}

// let new_service = make_service_fn(move |_| {
//     let root_node = schema.clone();
//     let ctx = context.clone();

//     async {
//         Ok::<_, hyper::Error>(service_fn(move |req| {
//             let root_node = root_node.clone();
//             let ctx = ctx.clone();

//             async {
//                 Ok::<_, Infallible>(match (req.method(), req.uri().path()) {
//                     (&Method::GET, "/") => juniper_hyper::graphiql("/graphql", None).await,
//                     (&Method::GET, "/graphql") | (&Method::POST, "/graphql") => {
//                         juniper_hyper::graphql(root_node, ctx, req).await
//                     }
//                     _ => {
//                         let mut response = Response::new(String::new());
//                         *response.status_mut() = StatusCode::NOT_FOUND;
//                         response
//                     }
//                 })
//             }
//         }));
//     }
// });

type Schema = RootNode<'static, Query, EmptyMutation<Context>, EmptySubscription<Context>>;

enum RequestError {
    Something(String),
}

impl IntoResponse for RequestError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Something(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
        }
    }
}

impl From<GraphQLError> for RequestError {
    fn from(value: GraphQLError) -> Self {
        Self::Something(value.to_string())
    }
}

async fn graphiql() -> Result<Html<String>, RequestError> {
    let text = graphiql_source("/graphql", None);
    Ok(Html(text))
}

#[derive(Deserialize)]
struct GraphQLRequest {
    query: String,
    variables: Option<HashMap<String, InputValue>>,
}

#[derive(Serialize)]
struct GraphQLResponse {
    data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<ExecutionError<DefaultScalarValue>>>,
}

impl GraphQLResponse {
    fn new(data: Value, errors: Option<Vec<ExecutionError<DefaultScalarValue>>>) -> Self {
        Self { data, errors }
    }
}

async fn execute_request(
    query: String,
    variables: Option<HashMap<String, InputValue>>,
    schema: &Schema,
    context: &Context,
) -> Result<Json<GraphQLResponse>, RequestError> {
    let vars = variables.unwrap_or_else(|| HashMap::new());
    let (result, errors) = juniper::execute(&query, None, &schema, &vars, &context).await?;

    Ok(Json(GraphQLResponse::new(
        result,
        if errors.len() == 0 {
            None
        } else {
            Some(errors)
        },
    )))
}

async fn graphql_post(
    State(RequestState { schema, context }): State<RequestState>,
    Json(body): Json<GraphQLRequest>,
) -> Result<Json<GraphQLResponse>, RequestError> {
    let GraphQLRequest { query, variables } = body;
    execute_request(query, variables, &schema, &context).await
}

async fn graphql_get(
    State(RequestState { schema, context }): State<RequestState>,
    axum::extract::Query(params): axum::extract::Query<GraphQLRequest>,
) -> Result<Json<GraphQLResponse>, RequestError> {
    let GraphQLRequest { query, variables } = params;
    execute_request(query, variables, &schema, &context).await
}

#[derive(Clone)]
struct RequestState {
    context: Arc<Context>,
    schema: Arc<Schema>,
}

impl RequestState {
    pub fn new(context: Arc<Context>, schema: Arc<Schema>) -> Self {
        Self { context, schema }
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // pretty_env_logger::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let context = Arc::new(Context::new());
    let schema = Arc::new(RootNode::new(
        Query,
        EmptyMutation::<Context>::new(),
        EmptySubscription::<Context>::new(),
    ));

    let service = Router::new()
        .route("/", get(graphiql))
        .route("/graphql", post(graphql_post).get(graphql_get))
        .with_state(RequestState::new(context, schema))
        .into_make_service();

    axum::Server::bind(&addr).serve(service).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use juniper::DefaultScalarValue;

    use super::*;
    #[test]
    fn test() {
        let context = Context::new();
        let schema = RootNode::new(
            Query,
            EmptyMutation::<Context>::new(),
            EmptySubscription::<Context>::new(),
        );

        println!("{}", schema.as_schema_language());

        let viewer_id = ViewerID(1);
        if let Value::Scalar(v) = to_id_output::<DefaultScalarValue>(&viewer_id) {
            println!("{}", v); // Vmlld2VyOjE=
            let input_value = InputValue::<DefaultScalarValue>::scalar(v);
            println!("{:?}", ViewerID::from_input(&input_value).unwrap())
        }

        let result = juniper::execute_sync(
            "query GetViewerQuery { node(id: 1) { id } }",
            None,
            &schema,
            &HashMap::new(),
            &context,
        );

        println!(
            "{}",
            serde_json::to_string_pretty(&result.unwrap().0).unwrap()
        );
    }
}
