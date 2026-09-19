// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Real AWS SDK v3 integration test: a genuine @aws-sdk/client-dynamodb client,
// with no network, talking to the ExtendDB wasm engine via the requestHandler
// shim. This is the POC's headline claim, validated end to end.
//
//   (build first: scripts/wasm-build.sh --target nodejs --dev)
//   cd crates/wasm/tests-node && npm install && node sdk-integration.mjs

import assert from "node:assert";
import { createRequire } from "node:module";

import {
  DynamoDBClient,
  CreateTableCommand,
  PutItemCommand,
  GetItemCommand,
  QueryCommand,
  DeleteItemCommand,
  BatchWriteItemCommand,
  BatchGetItemCommand,
  ConditionalCheckFailedException,
} from "@aws-sdk/client-dynamodb";

import { createExtenddbRequestHandler, nodeBody } from "../web/sdk-request-handler.mjs";

// The wasm pkg is a CommonJS module (wasm-pack --target nodejs).
const require = createRequire(import.meta.url);
const wasm = require("../pkg/extenddb_wasm.js");

wasm.init();

const client = new DynamoDBClient({
  region: "us-east-1",
  credentials: { accessKeyId: "AKIDEXAMPLE", secretAccessKey: "secret" },
  endpoint: "http://localhost:8000",
  // Fail fast: the engine is deterministic, so a 5xx (e.g. not-initialized)
  // should surface immediately rather than be retried up to maxAttempts.
  maxAttempts: 1,
  requestHandler: createExtenddbRequestHandler(wasm, { makeBody: nodeBody }),
});

// CreateTable (numeric partition key + string sort key).
await client.send(
  new CreateTableCommand({
    TableName: "Movies",
    KeySchema: [
      { AttributeName: "year", KeyType: "HASH" },
      { AttributeName: "title", KeyType: "RANGE" },
    ],
    AttributeDefinitions: [
      { AttributeName: "year", AttributeType: "N" },
      { AttributeName: "title", AttributeType: "S" },
    ],
    BillingMode: "PAY_PER_REQUEST",
  }),
);

// PutItem + GetItem round-trip through the real SDK marshalling.
await client.send(
  new PutItemCommand({
    TableName: "Movies",
    Item: { year: { N: "2013" }, title: { S: "Rush" }, rank: { N: "6" } },
  }),
);
await client.send(
  new PutItemCommand({
    TableName: "Movies",
    Item: { year: { N: "2013" }, title: { S: "Prisoners" }, rank: { N: "3" } },
  }),
);

const got = await client.send(
  new GetItemCommand({
    TableName: "Movies",
    Key: { year: { N: "2013" }, title: { S: "Rush" } },
  }),
);
assert.strictEqual(got.Item.rank.N, "6", "GetItem via SDK should return rank=6");

// Query one partition, ascending by sort key.
const q = await client.send(
  new QueryCommand({
    TableName: "Movies",
    KeyConditionExpression: "#y = :y",
    ExpressionAttributeNames: { "#y": "year" },
    ExpressionAttributeValues: { ":y": { N: "2013" } },
  }),
);
assert.strictEqual(q.Items.length, 2, "Query should return 2 items");
assert.strictEqual(q.Items[0].title.S, "Prisoners", "ascending sort order");
assert.strictEqual(q.Items[1].title.S, "Rush");

// Conditional write failure must surface as the typed SDK exception.
let threw = false;
try {
  await client.send(
    new PutItemCommand({
      TableName: "Movies",
      Item: { year: { N: "2013" }, title: { S: "Rush" }, rank: { N: "1" } },
      ConditionExpression: "attribute_not_exists(title)",
    }),
  );
} catch (e) {
  threw = true;
  assert.ok(
    e instanceof ConditionalCheckFailedException,
    "expected ConditionalCheckFailedException, got " + e.name,
  );
}
assert.ok(threw, "conditional PutItem should have thrown");

// Batch: one BatchWriteItem (two puts, one delete) then one BatchGetItem over
// the written keys plus the deleted one, through the real SDK marshalling.
const bw = await client.send(
  new BatchWriteItemCommand({
    RequestItems: {
      Movies: [
        { PutRequest: { Item: { year: { N: "2014" }, title: { S: "Interstellar" }, rank: { N: "1" } } } },
        { PutRequest: { Item: { year: { N: "2014" }, title: { S: "Whiplash" }, rank: { N: "2" } } } },
        { DeleteRequest: { Key: { year: { N: "2013" }, title: { S: "Prisoners" } } } },
      ],
    },
    ReturnConsumedCapacity: "TOTAL",
  }),
);
assert.deepStrictEqual(bw.UnprocessedItems, {}, "BatchWriteItem should leave nothing unprocessed");
assert.strictEqual(bw.ConsumedCapacity.length, 1, "one ConsumedCapacity entry for the one table");
assert.strictEqual(bw.ConsumedCapacity[0].TableName, "Movies");
assert.strictEqual(bw.ConsumedCapacity[0].CapacityUnits, 3, "three write units for three requests");

const bg = await client.send(
  new BatchGetItemCommand({
    RequestItems: {
      Movies: {
        Keys: [
          { year: { N: "2014" }, title: { S: "Interstellar" } },
          { year: { N: "2014" }, title: { S: "Whiplash" } },
          { year: { N: "2013" }, title: { S: "Prisoners" } },
        ],
        ProjectionExpression: "title, #r",
        ExpressionAttributeNames: { "#r": "rank" },
      },
    },
  }),
);
assert.deepStrictEqual(bg.UnprocessedKeys, {}, "BatchGetItem should leave nothing unprocessed");
const gotTitles = bg.Responses.Movies.map((it) => it.title.S).sort();
assert.deepStrictEqual(gotTitles, ["Interstellar", "Whiplash"], "the two puts are readable, the deleted key is absent");
for (const it of bg.Responses.Movies) {
  assert.deepStrictEqual(Object.keys(it).sort(), ["rank", "title"], "ProjectionExpression applied per item");
}

console.log(
  "SDK-v3 integration test PASSED: real @aws-sdk/client-dynamodb (Create/Put/Get/Query + conditional + BatchWrite/BatchGet) -> wasm engine, zero network",
);
