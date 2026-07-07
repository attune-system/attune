#!/bin/bash
# Generate Python client from OpenAPI spec
# By default this script exports OpenAPI directly from current source code,
# but it can also consume an explicit local spec path.

set -e

# Configuration
API_URL="${ATTUNE_API_URL:-http://localhost:8080}"
OPENAPI_SPEC_URL="${API_URL}/api-spec/openapi.json"
OUTPUT_DIR="tests/generated_client"
TEMP_SPEC="${OPENAPI_SPEC_PATH:-/tmp/attune-openapi.json}"
SKIP_CLIENT_INSTALL="${SKIP_CLIENT_INSTALL:-0}"
USE_RUNNING_API="${USE_RUNNING_API:-0}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Attune Python Client Generator ===${NC}"
echo ""

# Check if openapi-python-client is installed
OPENAPI_CLIENT_CMD="tests/venvs/e2e/bin/openapi-python-client"
if [ ! -f "${OPENAPI_CLIENT_CMD}" ]; then
    echo -e "${YELLOW}openapi-python-client not found. Installing...${NC}"
    if [ -d "tests/venvs/e2e" ]; then
        tests/venvs/e2e/bin/pip install openapi-python-client
    else
        echo -e "${RED}ERROR: E2E venv not found. Please create it first.${NC}"
        exit 1
    fi
    echo ""
fi

if [ "${USE_RUNNING_API}" = "1" ]; then
    # Check if API is running
    echo -e "${BLUE}Checking API availability at ${API_URL}...${NC}"
    if ! curl -s -f "${API_URL}/health" > /dev/null; then
        echo -e "${RED}ERROR: API is not running at ${API_URL}${NC}"
        echo "Please start the API service first:"
        echo "  cd tests && ./start_e2e_services.sh"
        exit 1
    fi
    echo -e "${GREEN}✓ API is running${NC}"
    echo ""

    # Download OpenAPI spec
    echo -e "${BLUE}Downloading OpenAPI spec from ${OPENAPI_SPEC_URL}...${NC}"
    if ! curl -s -f "${OPENAPI_SPEC_URL}" -o "${TEMP_SPEC}"; then
        echo -e "${RED}ERROR: Failed to download OpenAPI spec${NC}"
        echo "Make sure the API is running and the spec endpoint is available"
        exit 1
    fi
    echo -e "${GREEN}✓ OpenAPI spec downloaded${NC}"
    echo ""
elif [ -f "${TEMP_SPEC}" ] && [ "${OPENAPI_SPEC_PATH:-}" != "" ]; then
    echo -e "${GREEN}✓ Using provided local OpenAPI spec at ${TEMP_SPEC}${NC}"
    echo ""
else
    echo -e "${BLUE}Exporting OpenAPI spec from current source code...${NC}"
    cargo run --quiet -p attune-api --bin export-openapi -- "${TEMP_SPEC}"
    echo -e "${GREEN}✓ OpenAPI spec exported to ${TEMP_SPEC}${NC}"
    echo ""
fi

# Validate JSON
echo -e "${BLUE}Validating OpenAPI spec...${NC}"
if ! jq empty "${TEMP_SPEC}" 2>/dev/null; then
    echo -e "${RED}ERROR: Invalid JSON in OpenAPI spec${NC}"
    cat "${TEMP_SPEC}"
    exit 1
fi
echo -e "${GREEN}✓ OpenAPI spec is valid${NC}"
echo ""

# Show spec info
SPEC_TITLE=$(jq -r '.info.title' "${TEMP_SPEC}")
SPEC_VERSION=$(jq -r '.info.version' "${TEMP_SPEC}")
PATH_COUNT=$(jq '.paths | length' "${TEMP_SPEC}")
echo -e "${BLUE}API Info:${NC}"
echo "  Title: ${SPEC_TITLE}"
echo "  Version: ${SPEC_VERSION}"
echo "  Endpoints: ${PATH_COUNT}"
echo ""

# Remove old generated client if it exists
if [ -d "${OUTPUT_DIR}" ]; then
    echo -e "${YELLOW}Removing old generated client...${NC}"
    rm -rf "${OUTPUT_DIR}"
fi

# Generate Python client
echo -e "${BLUE}Generating Python client...${NC}"
"${OPENAPI_CLIENT_CMD}" generate \
    --path "${TEMP_SPEC}" \
    --output-path "${OUTPUT_DIR}" \
    --overwrite \
    --meta none

if [ $? -ne 0 ]; then
    echo -e "${RED}ERROR: Client generation failed${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Python client generated${NC}"
echo ""

# Install the generated client (only when metadata exists)
echo -e "${BLUE}Installing generated client into E2E venv...${NC}"
if [ "${SKIP_CLIENT_INSTALL}" = "1" ]; then
    echo -e "${YELLOW}Skipping client installation (SKIP_CLIENT_INSTALL=1)${NC}"
elif [ ! -f "${OUTPUT_DIR}/pyproject.toml" ] && [ ! -f "${OUTPUT_DIR}/setup.py" ]; then
    echo -e "${YELLOW}Skipping installation: ${OUTPUT_DIR} has no pyproject.toml/setup.py (generator --meta none)${NC}"
elif [ -d "tests/venvs/e2e" ]; then
    tests/venvs/e2e/bin/pip install -e "${OUTPUT_DIR}" --quiet
    echo -e "${GREEN}✓ Client installed${NC}"
else
    echo -e "${YELLOW}WARNING: E2E venv not found, skipping installation${NC}"
    echo "Run this to install manually:"
    echo "  tests/venvs/e2e/bin/pip install -e ${OUTPUT_DIR}"
fi
echo ""

# Clean up temporary spec only when this script generated/downloaded it
if [ "${OPENAPI_SPEC_PATH:-}" = "" ]; then
    rm -f "${TEMP_SPEC}"
fi

# Create a simple usage example
cat > "${OUTPUT_DIR}/USAGE.md" << 'EOF'
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
EOF

echo -e "${GREEN}=== Client Generation Complete ===${NC}"
echo ""
echo "Generated client location: ${OUTPUT_DIR}"
echo "Usage guide: ${OUTPUT_DIR}/USAGE.md"
echo ""
echo "To use the client in tests:"
echo "  from attune_client import Client"
echo "  from attune_client.api.packs import list_packs"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo "  1. Review the generated client in ${OUTPUT_DIR}"
echo "  2. Update test fixtures to use the generated client"
echo "  3. Remove manual client code in tests/helpers/client.py"
echo ""
