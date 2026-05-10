# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0

"""Phase 2 item operations tests — dual-target against real DynamoDB and extenddb.

Covers: PutItem, GetItem, composite keys, and all DynamoDB attribute types.
REQ-TEST-001, REQ-TEST-002, REQ-TEST-003, REQ-TEST-004
"""

from __future__ import annotations

import uuid

import pytest
from botocore.exceptions import ClientError

from conftest import wait_for_active
@pytest.fixture()
def hash_table(dynamodb_client, create_and_cleanup_table, unique_table_name):
    """Create a hash-only table and wait for ACTIVE."""
    create_and_cleanup_table(unique_table_name)
    wait_for_active(dynamodb_client, unique_table_name)
    return unique_table_name
@pytest.fixture()
def hash_range_table(dynamodb_client, create_and_cleanup_table):
    """Create a hash+range (S,S) table and wait for ACTIVE."""
    name = f"extenddb-test-{uuid.uuid4().hex[:12]}"
    create_and_cleanup_table(
        name,
        AttributeDefinitions=[
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "S"},
        ],
        KeySchema=[
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
    )
    wait_for_active(dynamodb_client, name)
    return name
@pytest.fixture()
def hash_numeric_range_table(dynamodb_client, create_and_cleanup_table):
    """Create a hash+range (S,N) table and wait for ACTIVE."""
    name = f"extenddb-test-{uuid.uuid4().hex[:12]}"
    create_and_cleanup_table(
        name,
        AttributeDefinitions=[
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "N"},
        ],
        KeySchema=[
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
    )
    wait_for_active(dynamodb_client, name)
    return name
class TestPutItem:
    """PutItem operation tests."""

    def test_put_and_get_simple_item(self, dynamodb_client, hash_table):
        item = {"pk": {"S": "user-1"}, "name": {"S": "Alice"}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "user-1"}})
        assert resp["Item"] == item

    def test_put_all_attribute_types(self, dynamodb_client, hash_table):
        raw_bytes = b"\x00\x01\x02\xff"
        item = {
            "pk": {"S": "types-test"},
            "str_attr": {"S": "hello"},
            "num_attr": {"N": "42.5"},
            "bin_attr": {"B": raw_bytes},
            "bool_attr": {"BOOL": True},
            "null_attr": {"NULL": True},
            "list_attr": {"L": [{"S": "a"}, {"N": "1"}]},
            "map_attr": {"M": {"nested": {"S": "value"}}},
            "ss_attr": {"SS": ["a", "b", "c"]},
            "ns_attr": {"NS": ["1", "2", "3"]},
            "bs_attr": {"BS": [b"\x01", b"\x02"]},
        }
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "types-test"}})
        got = resp["Item"]

        assert got["pk"] == item["pk"]
        assert got["str_attr"] == item["str_attr"]
        assert got["num_attr"] == item["num_attr"]
        assert got["bin_attr"]["B"] == raw_bytes
        assert got["bool_attr"] == item["bool_attr"]
        assert got["null_attr"] == item["null_attr"]
        assert got["list_attr"] == item["list_attr"]
        assert got["map_attr"] == item["map_attr"]
        # Sets: DynamoDB does not guarantee order, so compare as sets
        assert set(got["ss_attr"]["SS"]) == set(item["ss_attr"]["SS"])
        assert set(got["ns_attr"]["NS"]) == set(item["ns_attr"]["NS"])
        assert set(got["bs_attr"]["BS"]) == {b"\x01", b"\x02"}

    def test_put_return_values_all_old_new_item(self, dynamodb_client, hash_table):
        resp = dynamodb_client.put_item(
            TableName=hash_table,
            Item={"pk": {"S": "new-item"}},
            ReturnValues="ALL_OLD",
        )
        assert "Attributes" not in resp

    def test_put_return_values_all_old_overwrite(self, dynamodb_client, hash_table):
        old_item = {"pk": {"S": "overwrite-me"}, "v": {"N": "1"}}
        dynamodb_client.put_item(TableName=hash_table, Item=old_item)
        resp = dynamodb_client.put_item(
            TableName=hash_table,
            Item={"pk": {"S": "overwrite-me"}, "v": {"N": "2"}},
            ReturnValues="ALL_OLD",
        )
        assert resp["Attributes"] == old_item

    def test_put_return_values_none_explicit(self, dynamodb_client, hash_table):
        resp = dynamodb_client.put_item(
            TableName=hash_table,
            Item={"pk": {"S": "none-rv"}},
            ReturnValues="NONE",
        )
        assert "Attributes" not in resp

    def test_put_invalid_return_values(self, dynamodb_client, hash_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.put_item(
                TableName=hash_table,
                Item={"pk": {"S": "x"}},
                ReturnValues="ALL_NEW",
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_put_item_exceeds_400kb(self, dynamodb_client, hash_table):
        big_item = {"pk": {"S": "big"}, "data": {"S": "x" * 409_601}}
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.put_item(TableName=hash_table, Item=big_item)
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_put_wrong_key_type(self, dynamodb_client, hash_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.put_item(
                TableName=hash_table,
                Item={"pk": {"N": "123"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_put_missing_key_attribute(self, dynamodb_client, hash_range_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.put_item(
                TableName=hash_range_table,
                Item={"pk": {"S": "only-hash"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_put_nonexistent_table(self, dynamodb_client):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.put_item(
                TableName="nonexistent-table-xyz-999",
                Item={"pk": {"S": "x"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ResourceNotFoundException"
class TestGetItem:
    """GetItem operation tests."""

    def test_get_existing_item(self, dynamodb_client, hash_table):
        item = {"pk": {"S": "get-me"}, "data": {"S": "found"}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "get-me"}})
        assert resp["Item"] == item

    def test_get_nonexistent_item(self, dynamodb_client, hash_table):
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "no-such-key"}})
        assert "Item" not in resp

    def test_get_nonexistent_table(self, dynamodb_client):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.get_item(
                TableName="nonexistent-table-xyz-999",
                Key={"pk": {"S": "x"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ResourceNotFoundException"

    def test_get_extra_attributes_in_key(self, dynamodb_client, hash_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.get_item(
                TableName=hash_table,
                Key={"pk": {"S": "x"}, "extra": {"S": "y"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_get_missing_key_attribute(self, dynamodb_client, hash_range_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.get_item(
                TableName=hash_range_table,
                Key={"pk": {"S": "x"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"

    def test_get_wrong_key_type(self, dynamodb_client, hash_table):
        with pytest.raises(ClientError) as exc_info:
            dynamodb_client.get_item(
                TableName=hash_table,
                Key={"pk": {"N": "123"}},
            )
        assert exc_info.value.response["Error"]["Code"] == "ValidationException"
class TestCompositeKey:
    """Tests for hash+range key tables."""

    def test_put_get_string_range(self, dynamodb_client, hash_range_table):
        item = {"pk": {"S": "user-1"}, "sk": {"S": "profile"}, "data": {"S": "hello"}}
        dynamodb_client.put_item(TableName=hash_range_table, Item=item)
        resp = dynamodb_client.get_item(
            TableName=hash_range_table,
            Key={"pk": {"S": "user-1"}, "sk": {"S": "profile"}},
        )
        assert resp["Item"] == item

    def test_put_get_numeric_range(self, dynamodb_client, hash_numeric_range_table):
        item = {"pk": {"S": "user-1"}, "sk": {"N": "100"}, "data": {"S": "hello"}}
        dynamodb_client.put_item(TableName=hash_numeric_range_table, Item=item)
        resp = dynamodb_client.get_item(
            TableName=hash_numeric_range_table,
            Key={"pk": {"S": "user-1"}, "sk": {"N": "100"}},
        )
        assert resp["Item"] == item

    def test_same_hash_different_range(self, dynamodb_client, hash_range_table):
        item_a = {"pk": {"S": "user-1"}, "sk": {"S": "email"}, "v": {"S": "a"}}
        item_b = {"pk": {"S": "user-1"}, "sk": {"S": "phone"}, "v": {"S": "b"}}
        dynamodb_client.put_item(TableName=hash_range_table, Item=item_a)
        dynamodb_client.put_item(TableName=hash_range_table, Item=item_b)

        resp_a = dynamodb_client.get_item(
            TableName=hash_range_table,
            Key={"pk": {"S": "user-1"}, "sk": {"S": "email"}},
        )
        resp_b = dynamodb_client.get_item(
            TableName=hash_range_table,
            Key={"pk": {"S": "user-1"}, "sk": {"S": "phone"}},
        )
        assert resp_a["Item"] == item_a
        assert resp_b["Item"] == item_b
class TestDataTypes:
    """Edge cases for DynamoDB attribute type fidelity."""

    def test_empty_string_value(self, dynamodb_client, hash_table):
        item = {"pk": {"S": "empty-str"}, "val": {"S": ""}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "empty-str"}})
        assert resp["Item"]["val"] == {"S": ""}

    def test_empty_binary_value(self, dynamodb_client, hash_table):
        item = {"pk": {"S": "empty-bin"}, "val": {"B": b""}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "empty-bin"}})
        assert resp["Item"]["val"]["B"] == b""

    def test_unicode_characters(self, dynamodb_client, hash_table):
        item = {"pk": {"S": "unicode"}, "emoji": {"S": "🎉🚀"}, "cjk": {"S": "日本語"}, "combining": {"S": "é"}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "unicode"}})
        assert resp["Item"]["emoji"] == {"S": "🎉🚀"}
        assert resp["Item"]["cjk"] == {"S": "日本語"}
        assert resp["Item"]["combining"] == {"S": "é"}

    def test_large_number(self, dynamodb_client, hash_table):
        # DynamoDB supports up to 38 significant digits
        big_num = "12345678901234567890123456789012345678"
        item = {"pk": {"S": "big-num"}, "val": {"N": big_num}}
        dynamodb_client.put_item(TableName=hash_table, Item=item)
        resp = dynamodb_client.get_item(TableName=hash_table, Key={"pk": {"S": "big-num"}})
        assert resp["Item"]["val"]["N"] == big_num
