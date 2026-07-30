# Invalid JSON

HTTP status: `400 Bad Request`

The request body could not be parsed as JSON, or it did not match the expected schema for the endpoint. Verify:

- The `Content-Type` header is `application/json`.
- The body is well-formed JSON.
- Required fields and data types match the operation's request schema.
