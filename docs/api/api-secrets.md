# Secret Management API

The Secret Management API provides secure endpoints for storing, retrieving, and managing sensitive credentials, API keys, tokens, and other secret values in the Attune automation platform. All secret values are encrypted at rest using AES-256-GCM encryption.

## Table of Contents

- [Overview](#overview)
- [Security Model](#security-model)
- [Key Model](#key-model)
- [Authentication](#authentication)
- [Endpoints](#endpoints)
  - [List Keys](#list-keys)
  - [Get Key by Reference](#get-key-by-reference)
  - [Create Key](#create-key)
  - [Update Key](#update-key)
  - [Delete Key](#delete-key)
- [Use Cases](#use-cases)
- [Security Best Practices](#security-best-practices)
- [Related Resources](#related-resources)

---

## Overview

The Secret Management API enables secure storage and retrieval of sensitive data that actions and sensors need to execute. Common use cases include:

- **API Credentials**: Store API keys, tokens, and secrets for external services
- **Database Credentials**: Store database passwords and connection strings
- **SSH Keys**: Store private SSH keys for remote access
- **OAuth Tokens**: Store OAuth access and refresh tokens
- **Service Account Keys**: Store service account credentials for cloud providers

### Key Features

- **Encryption at Rest**: All secret values are encrypted using AES-256-GCM
- **Value Redaction**: List views never expose actual secret values
- **Owner Association**: Link secrets to identities, packs, actions, or sensors
- **Audit Trail**: Track creation and modification timestamps
- **Flexible Ownership**: Support multiple ownership models (system, identity, pack, action, sensor)

---

## Security Model

### Encryption

All secret values marked as `encrypted: true` are encrypted using AES-256-GCM encryption before being stored in the database. The encryption process:

1. **Key Derivation**: The server's encryption key is hashed using SHA-256 to derive a 256-bit AES key
2. **Random Nonce**: A random 96-bit nonce is generated for each encryption operation
3. **Encryption**: The plaintext is encrypted using AES-256-GCM with the derived key and nonce
4. **Storage**: The encrypted value (nonce + ciphertext + authentication tag) is base64-encoded and stored

### Decryption

When retrieving a secret value via GET `/api/v1/keys/:ref`, the server automatically decrypts the value if it's encrypted:

1. The encrypted value is base64-decoded
2. The nonce is extracted from the beginning of the data
3. The ciphertext is decrypted using the server's encryption key
4. The decrypted plaintext is returned in the API response

### Access Control

- **Authentication Required**: All endpoints require JWT authentication
- **No List Value Exposure**: List endpoints (`GET /keys`) never return actual secret values
- **Individual Retrieval**: Secret values can only be retrieved one at a time via GET `/keys/:ref`
- **Audit Logging**: All access is logged (future enhancement)

### Server Configuration

The server must have an encryption key configured:

```yaml
security:
  encryption_key: "your-encryption-key-must-be-at-least-32-characters-long"
```

Or via environment variable:

```bash
ATTUNE__SECURITY__ENCRYPTION_KEY="your-encryption-key-must-be-at-least-32-characters-long"
```

⚠️ **Warning**: The encryption key must be:
- At least 32 characters long
- Kept secret and secure
- Backed up securely
- Never committed to version control
- Rotated periodically (requires re-encrypting all secrets)

---

## Key Model

### Key Object

```json
{
  "id": 123,
  "ref": "pack.github.github_api_token",
  "local_ref": "github_api_token",
  "owner_type": "pack",
  "owner": "github",
  "owner_identity": null,
  "owner_pack": 456,
  "owner_pack_ref": "github",
  "owner_action": null,
  "owner_action_ref": null,
  "owner_sensor": null,
  "owner_sensor_ref": null,
  "name": "GitHub Personal Access Token",
  "encrypted": true,
  "value": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
  "created": "2024-01-15T10:00:00Z",
  "updated": "2024-01-15T10:00:00Z"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique key identifier |
| `ref` | string | Canonical owner-qualified reference, such as `pack.github.github_token` |
| `local_ref` | string | Identifier within the owner scope, such as `github_token` |
| `owner_type` | string | Owner type: `system`, `identity`, `pack`, `action`, `sensor` |
| `owner` | string | Optional owner string identifier |
| `owner_identity` | integer | Optional owner identity ID |
| `owner_pack` | integer | Optional owner pack ID |
| `owner_pack_ref` | string | Optional owner pack reference |
| `owner_action` | integer | Optional owner action ID |
| `owner_action_ref` | string | Optional owner action reference |
| `owner_sensor` | integer | Optional owner sensor ID |
| `owner_sensor_ref` | string | Optional owner sensor reference |
| `name` | string | Human-readable name |
| `encrypted` | boolean | Whether the value is encrypted (recommended: true) |
| `value` | string | The secret value (decrypted in single-item GET, omitted in lists) |
| `created` | datetime | Timestamp when key was created |
| `updated` | datetime | Timestamp of last update |

### Owner Types

| Type | Description | Use Case |
|------|-------------|----------|
| `system` | System-wide secret | Global configuration, shared credentials |
| `identity` | User-owned secret | Personal API keys, user-specific tokens |
| `pack` | Pack-scoped secret | Credentials for a specific integration pack |
| `action` | Action-specific secret | Credentials used by a single action |
| `sensor` | Sensor-specific secret | Credentials used by a sensor |

Canonical refs use these forms:

- `system.<local_ref>`
- `identity.<login>.<local_ref>`
- `pack.<pack-ref>.<local_ref>`
- `action.<action-ref>.<local_ref>`
- `sensor.<sensor-ref>.<local_ref>`

Create requests provide `local_ref` and the matching owner selector. The server
resolves the owner and constructs `ref`; clients must not construct a different
owner-qualified ref.

---

## Authentication

All secret management endpoints require authentication. Include a valid JWT access token in the `Authorization` header:

```
Authorization: Bearer <access_token>
```

See the [Authentication Guide](./authentication.md) for details on obtaining tokens.

---

## Endpoints

### List Keys

Retrieve a paginated list of keys with optional filtering. **Values are redacted** in list views for security.

**Endpoint:** `GET /api/v1/keys`

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `owner_type` | string | - | Filter by owner type (system, identity, pack, action, sensor) |
| `owner` | string | - | Filter by owner string |
| `page` | integer | 1 | Page number (1-indexed) |
| `per_page` | integer | 50 | Items per page (max 100) |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/keys?owner_type=pack&page=1" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 123,
      "ref": "github_api_token",
      "owner_type": "pack",
      "owner": "github-integration",
      "name": "GitHub Personal Access Token",
      "encrypted": true,
      "created": "2024-01-15T10:00:00Z"
    },
    {
      "id": 124,
      "ref": "aws_secret_key",
      "owner_type": "pack",
      "owner": "aws-integration",
      "name": "AWS Secret Access Key",
      "encrypted": true,
      "created": "2024-01-15T11:00:00Z"
    }
  ],
  "pagination": {
    "page": 1,
    "page_size": 50,
    "total_items": 2,
    "total_pages": 1
  }
}
```

**Note**: Secret values are **never** included in list responses for security.

---

### Get Key by Reference

Retrieve a single key by its reference. **The secret value is decrypted and returned** in the response.

**Endpoint:** `GET /api/v1/keys/:ref`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `ref` | string | Canonical key reference, such as `pack.github.github_api_token` |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/keys/pack.github.github_api_token" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "ref": "pack.github.github_api_token",
    "local_ref": "github_api_token",
    "owner_type": "pack",
    "owner": "github-integration",
    "owner_identity": null,
    "owner_pack": 456,
    "owner_pack_ref": "github",
    "owner_action": null,
    "owner_action_ref": null,
    "owner_sensor": null,
    "owner_sensor_ref": null,
    "name": "GitHub Personal Access Token",
    "encrypted": true,
    "value": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:00:00Z"
  }
}
```

**Security Note**: The `value` field contains the **decrypted plaintext** secret. Handle this data carefully and never log or expose it.

**Error Responses:**

- `404 Not Found`: Key not found
- `500 Internal Server Error`: Decryption failed (wrong encryption key or corrupted data)

---

### Create Key

Create a key. Set `encrypted` to `true` to encrypt its value at rest.

**Endpoint:** `POST /api/v1/keys`

**Request Body:**

```json
{
  "local_ref": "github_api_token",
  "owner_type": "pack",
  "owner_pack_ref": "github",
  "name": "GitHub Personal Access Token",
  "value": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
  "encrypted": true
}
```

**Field Validation:**

| Field | Required | Constraints |
|-------|----------|-------------|
| `local_ref` | Yes | 1-63 bytes; lowercase letters, numbers, underscores, and hyphens; must start with a letter or number |
| `owner_type` | Yes | Must be: system, identity, pack, action, sensor |
| `owner_identity_login` | For identity keys | Existing identity login |
| `owner_pack_ref` | No | Max 255 characters |
| `owner_action_ref` | No | Max 255 characters |
| `owner_sensor_ref` | No | Max 255 characters |
| `name` | Yes | 1-255 characters |
| `value` | Yes | 1-10,000 characters |
| `encrypted` | No | Boolean (default: true) |

**Example Request:**

```bash
curl -X POST "http://localhost:8080/api/v1/keys" \
  -H "Authorization: Bearer <access_token>" \
  -H "Content-Type: application/json" \
  -d '{
    "local_ref": "github_api_token",
    "owner_type": "pack",
    "owner_pack_ref": "github",
    "name": "GitHub Personal Access Token",
    "value": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
    "encrypted": true
  }'
```

**Response:** `201 Created`

```json
{
  "data": {
    "id": 123,
    "ref": "pack.github.github_api_token",
    "local_ref": "github_api_token",
    "owner_type": "pack",
    "owner": "github",
    "owner_identity": null,
    "owner_pack": null,
    "owner_pack_ref": "github",
    "owner_action": null,
    "owner_action_ref": null,
    "owner_sensor": null,
    "owner_sensor_ref": null,
    "name": "GitHub Personal Access Token",
    "encrypted": true,
    "value": "ghp_1234567890abcdefghijklmnopqrstuvwxyz",
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:00:00Z"
  },
  "message": "Key created successfully"
}
```

**Error Responses:**

- `400 Bad Request`: Validation error or encryption key not configured
- `409 Conflict`: Key with the same canonical `ref` already exists

---

### Update Key

Update an existing key's name or value. If the value is updated and encryption is enabled, it will be re-encrypted.

**Endpoint:** `PUT /api/v1/keys/:ref`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `ref` | string | Canonical key reference |

**Request Body:**

```json
{
  "name": "GitHub Token (Updated)",
  "value": "ghp_newtoken123456789abcdefghijklmnopqr",
  "encrypted": true
}
```

**Updatable Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Update the human-readable name |
| `value` | string | Update the secret value (will be re-encrypted if needed) |
| `encrypted` | boolean | Change encryption status (re-encrypts/decrypts value) |

**Example Request:**

```bash
curl -X PUT "http://localhost:8080/api/v1/keys/pack.github.github_api_token" \
  -H "Authorization: Bearer <access_token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "GitHub Token (Production)",
    "value": "ghp_newtoken123456789abcdefghijklmnopqr"
  }'
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "ref": "pack.github.github_api_token",
    "local_ref": "github_api_token",
    "owner_type": "pack",
    "owner": "github-integration",
    "owner_identity": null,
    "owner_pack": 456,
    "owner_pack_ref": "github",
    "owner_action": null,
    "owner_action_ref": null,
    "owner_sensor": null,
    "owner_sensor_ref": null,
    "name": "GitHub Token (Production)",
    "encrypted": true,
    "value": "ghp_newtoken123456789abcdefghijklmnopqr",
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T14:30:00Z"
  },
  "message": "Key updated successfully"
}
```

**Error Responses:**

- `404 Not Found`: Key not found
- `400 Bad Request`: Validation error

---

### Delete Key

Delete a secret key permanently.

**Endpoint:** `DELETE /api/v1/keys/:ref`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `ref` | string | Canonical key reference |

**Example Request:**

```bash
curl -X DELETE "http://localhost:8080/api/v1/keys/pack.github.github_api_token" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "message": "Key deleted successfully",
  "success": true
}
```

**Error Responses:**

- `404 Not Found`: Key not found

---

## Use Cases

### Store API Credentials

Store third-party API credentials for use in actions:

```bash
curl -X POST "http://localhost:8080/api/v1/keys" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "sendgrid_api_key",
    "owner_type": "pack",
    "owner_pack_ref": "email",
    "name": "SendGrid API Key",
    "value": "SG.abcdefghijklmnopqrstuvwxyz",
    "encrypted": true
  }'
```

### Store Database Credentials

Store database connection credentials:

```bash
curl -X POST "http://localhost:8080/api/v1/keys" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "prod_db_password",
    "owner_type": "system",
    "name": "Production Database Password",
    "value": "supersecretpassword123!",
    "encrypted": true
  }'
```

### Store OAuth Tokens

Store OAuth access tokens for external services:

```bash
curl -X POST "http://localhost:8080/api/v1/keys" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "slack_oauth_token",
    "owner_type": "identity",
    "owner_identity": 789,
    "name": "Slack OAuth Token",
    "value": "xoxb-1234567890-abcdefghijklmnopqr",
    "encrypted": true
  }'
```

### Retrieve Secret for Action

Actions can retrieve secrets at runtime:

```bash
# Get secret value
SECRET_VALUE=$(curl -s -X GET "http://localhost:8080/api/v1/keys/github_api_token" \
  -H "Authorization: Bearer <token>" | jq -r '.data.value')

# Use in action
curl -X GET "https://api.github.com/user" \
  -H "Authorization: token $SECRET_VALUE"
```

### List Secrets by Owner

List all secrets owned by a specific pack:

```bash
curl -X GET "http://localhost:8080/api/v1/keys?owner_type=pack&owner=github-integration" \
  -H "Authorization: Bearer <token>"
```

### Update Expired Token

Update a secret when credentials change:

```bash
curl -X PUT "http://localhost:8080/api/v1/keys/github_api_token" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "value": "ghp_newtoken_after_rotation"
  }'
```

---

## Security Best Practices

### 1. Always Encrypt Sensitive Data

**Always** set `encrypted: true` when creating keys containing sensitive data:

```json
{
  "ref": "aws_secret_key",
  "name": "AWS Secret Access Key",
  "value": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
  "encrypted": true
}
```

### 2. Use Descriptive References

Use clear, descriptive references that indicate the purpose:

✅ Good:
- `github_api_token`
- `prod_db_password`
- `slack_oauth_token`

❌ Bad:
- `token1`
- `secret`
- `key`

### 3. Associate with Owners

Always associate secrets with appropriate owners for better organization:

```json
{
  "ref": "github_deploy_key",
  "owner_type": "action",
  "owner_action_ref": "deploy_to_production",
  "name": "GitHub Deployment Key",
  "value": "ssh-rsa AAAAB3NzaC1...",
  "encrypted": true
}
```

### 4. Rotate Secrets Regularly

Implement a secret rotation policy:

```bash
# Update secret with new value
curl -X PUT "http://localhost:8080/api/v1/keys/api_key" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"value": "new_rotated_value"}'
```

### 5. Never Log Secret Values

Never log or print secret values in application code:

```python
# ❌ BAD - Logs secret value
logger.info(f"Using API key: {secret_value}")

# ✅ GOOD - Logs redacted info
logger.info(f"Using API key: {secret_value[:4]}...{secret_value[-4:]}")
```

### 6. Limit Access

Use authentication and authorization to limit who can access secrets:

- Implement RBAC (future enhancement)
- Audit secret access (future enhancement)
- Rotate tokens regularly
- Use least-privilege principle

### 7. Backup Encryption Key

**Critical**: Securely backup your encryption key. If you lose it, encrypted secrets cannot be recovered:

```bash
# Backup encryption key to secure location
echo "$ATTUNE__SECURITY__ENCRYPTION_KEY" | gpg --encrypt > encryption_key.gpg.backup
```

### 8. Use Environment-Specific Secrets

Use different secrets for different environments:

- `dev_api_key` - Development environment
- `staging_api_key` - Staging environment
- `prod_api_key` - Production environment

---

## Error Handling

### Common Error Codes

| Status Code | Description |
|-------------|-------------|
| `400 Bad Request` | Invalid input, validation error, or encryption not configured |
| `401 Unauthorized` | Missing or invalid authentication token |
| `404 Not Found` | Key not found |
| `409 Conflict` | Key with same reference already exists |
| `500 Internal Server Error` | Encryption/decryption error or server error |

### Example Error Responses

**Encryption Key Not Configured:**

```json
{
  "error": "Cannot encrypt: encryption key not configured on server",
  "status": 400
}
```

**Key Already Exists:**

```json
{
  "error": "Key with ref 'github_api_token' already exists",
  "status": 409
}
```

**Decryption Failed:**

```json
{
  "error": "Failed to decrypt key: Decryption failed",
  "status": 500
}
```

---

## Encryption Details

### Algorithm

- **Cipher**: AES-256-GCM (Galois/Counter Mode)
- **Key Size**: 256 bits (32 bytes)
- **Nonce Size**: 96 bits (12 bytes)
- **Authentication**: Built-in AEAD authentication

### Key Derivation

The server's encryption key (configured in `security.encryption_key`) is hashed using SHA-256 to derive the actual AES-256 key:

```
AES_KEY = SHA256(encryption_key)
```

### Encrypted Value Format

Encrypted values are stored as base64-encoded strings containing:

```
BASE64(nonce || ciphertext || authentication_tag)
```

Where:
- `nonce`: 12 bytes (randomly generated for each encryption)
- `ciphertext`: Variable length (encrypted plaintext)
- `authentication_tag`: 16 bytes (GCM authentication tag)

### Security Properties

- **Confidentiality**: AES-256 encryption prevents unauthorized reading
- **Authenticity**: GCM mode prevents tampering and forgery
- **Non-deterministic**: Random nonces ensure same plaintext produces different ciphertexts
- **Forward Security**: Key rotation possible (requires re-encrypting all secrets)

---

## Related Resources

- [Action Management API](./api-actions.md) - Actions that use secrets
- [Pack Management API](./api-packs.md) - Packs that own secrets
- [Authentication Guide](./authentication.md) - API authentication details
- [Configuration Guide](./configuration.md) - Server configuration including encryption key

---

## Future Enhancements

### Planned Features

1. **Key Rotation**: Automatic re-encryption when changing encryption keys
2. **Access Control Lists**: Fine-grained permissions on who can access which secrets
3. **Audit Logging**: Detailed logs of all secret access and modifications
4. **Secret Expiration**: Time-to-live (TTL) for temporary secrets
5. **Secret Versioning**: Keep history of secret value changes
6. **Import/Export**: Secure import/export of secrets with encryption
7. **Secret References**: Reference secrets from other secrets
8. **Integration with Vaults**: Support for HashiCorp Vault, AWS Secrets Manager, etc.

---

**Last Updated:** 2024-01-13  
**API Version:** v1  
**Security Note:** Always use HTTPS in production to protect secrets in transit
