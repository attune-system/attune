# Attune Python Client Usage

This client was auto-generated from the Attune OpenAPI specification.

## Installation

```bash
pip install -e tests/generated_client
```

## Basic Usage

```python
from attune_client import Client
from attune_client.api.auth import login
from attune_client.models import LoginRequest

# Create client
client = Client(base_url="http://localhost:8080")

# Login
login_request = LoginRequest(
    login="test@attune.local",
    password="TestPass123!"
)

response = login.sync(client=client, json_body=login_request)
token = response.data.access_token

# Use authenticated client
client = Client(
    base_url="http://localhost:8080",
    token=token
)

# List packs
from attune_client.api.packs import list_packs
packs = list_packs.sync(client=client)
print(f"Found {len(packs.data)} packs")
```

## Async Usage

All API calls have async equivalents:

```python
import asyncio
from attune_client import Client
from attune_client.api.packs import list_packs

async def main():
    client = Client(base_url="http://localhost:8080", token="your-token")
    packs = await list_packs.asyncio(client=client)
    print(f"Found {len(packs.data)} packs")

asyncio.run(main())
```

## Regenerating

To regenerate the client after API changes:

```bash
./scripts/generate-python-client.sh
```
