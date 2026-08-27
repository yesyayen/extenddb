// Bundle entry: expose the real AWS SDK v3 DynamoDB client, the DynamoDB
// command classes, and the ExtendDB requestHandler shim as one browser global
// (window.ExtendDBSDK). esbuild builds this into web/pkg-sdk/extenddb-sdk.js.
//
// The in-page SDK console constructs a DynamoDBClient whose requestHandler is
// the shim, so `client.send(new QueryCommand(...))` runs entirely in the tab
// against the wasm engine, with zero network.

export * from "@aws-sdk/client-dynamodb";
export { createExtenddbRequestHandler, browserBody } from "../web/sdk-request-handler.mjs";
