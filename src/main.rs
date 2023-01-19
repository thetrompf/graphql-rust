use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;
use http_body_util::{BodyExt, Collected, Full, StreamBody};
use hyper::body::{Body, Incoming};
use hyper::service::Service;
use hyper::{body, Request};
use hyper::{server::conn::http1, service::service_fn, Method, Response, StatusCode};
use juniper::futures::future::Then;
use juniper::futures::FutureExt;
use juniper::{
    graphql_object, EmptyMutation, EmptySubscription, FieldResult, GraphQLObject, GraphQLScalar,
    InputValue, RootNode, ScalarValue, Value,
};
use std::future::Future;
use std::pin::Pin;
use std::{convert::Infallible, sync::Arc};
use std::{net::SocketAddr, str::Utf8Error};
use tokio::net::TcpListener;
struct Context;
impl Context {
    fn new() -> Self {
        Context {}
    }
}
impl juniper::Context for Context {}

#[derive(GraphQLScalar, Debug)]
#[graphql(name = "ID", to_output_with = to_id_output, transparent)]
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

struct HyperService {
    context: Arc<Context>,
    schema: Arc<RootNode<'static, Query, EmptyMutation<Context>, EmptySubscription<Context>>>,
}

impl Service<Request<Incoming>> for HyperService {
    type Response = Response<String>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let context = self.context.clone();
        let schema = self.schema.clone();
        Box::pin(async move {
            match (&parts.method, parts.uri.path()) {
                (&Method::GET, "/") => {
                    let result = juniper_hyper::graphiql("/graphql", None).await;

                    Ok(result)
                }
                (&Method::POST, "/graphql") => {
                    let body: Result<_, _> = body.collect().await;
                    let body = match body {
                        Ok(body) => body,
                        Err(err) => {
                            eprintln!("Error reading request body {:?}", err);
                            return Ok(Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body("Could not read request body".to_string())
                                .unwrap());
                        }
                    };
                    let body: Bytes = body.to_bytes();
                    let res = juniper_hyper::graphql(
                        schema,
                        context,
                        Request::from_parts(parts, body.into()),
                    )
                    .await;
                    Ok(res)
                }
                _ => Ok(Response::builder().body(String::new()).unwrap()),
            }
        })
    }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // pretty_env_logger::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let context = Arc::new(Context::new());
    let schema = Arc::new(RootNode::new(
        Query,
        EmptyMutation::<Context>::new(),
        EmptySubscription::<Context>::new(),
    ));

    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let root_node = schema.clone();
        let ctx = context.clone();

        tokio::task::spawn(async move {
            let root_node = root_node.clone();
            let ctx = ctx.clone();

            if let Err(err) = http1::Builder::new()
                .serve_connection(
                    stream,
                    HyperService {
                        context: ctx,
                        schema: root_node,
                    }, // let root_node = schema.clone();
                       // let ctx = context.clone();

                       // async {
                       //     Ok::<_, Infallible>(match (req.method(), req.uri().path()) {
                       // (&Method::GET, "/") => {
                       //     juniper_hyper::graphiql("/graphql", None).await
                       // }
                       // (&Method::GET, "/graphql") | (&Method::POST, "/graphql") => {
                       //     juniper_hyper::graphql(root_node, ctx, req).await
                       // }
                       // _ => {
                       // let mut response = Response::new(String::new());
                       // *response.status_mut() = StatusCode::NOT_FOUND;
                       // Ok(response)
                       // }
                       // })
                       // }
                       // }),
                )
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
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
