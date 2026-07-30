# Validation Error

HTTP status: `400 Bad Request`

The request body failed one or more validation rules. Common causes:

- Missing both `text` and `words`: at least one input field must be provided.
- `text` exceeds 10,000 characters.
- `words` contains more than 1,000 entries.

Check the response body for the RFC 7807 `detail` field.
