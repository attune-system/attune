#!/bin/bash
# CORS and Proxy Testing Script
# This script helps diagnose CORS and proxy configuration issues

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_header() {
    echo -e "\n${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}\n"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_info() {
    echo -e "${YELLOW}ℹ${NC} $1"
}

# Check if jq is installed
if ! command -v jq &> /dev/null; then
    print_error "jq is not installed. Install it for better JSON formatting."
    JQ_AVAILABLE=false
else
    JQ_AVAILABLE=true
fi

print_header "Attune CORS & Proxy Diagnostic Tool"

# Test 1: Check if API server is running
print_header "Test 1: API Server Availability"
if curl -s -f http://localhost:8080/health > /dev/null 2>&1; then
    print_success "API server is running on localhost:8080"

    if [ "$JQ_AVAILABLE" = true ]; then
        echo -e "\nHealth response:"
        curl -s http://localhost:8080/health | jq .
    fi
else
    print_error "API server is NOT running on localhost:8080"
    print_info "Start the API server with: ./scripts/start_services_test.sh"
    exit 1
fi

# Test 2: Check if Vite dev server is running
print_header "Test 2: Vite Dev Server Availability"
if curl -s -f http://localhost:3000 > /dev/null 2>&1; then
    print_success "Vite dev server is running on localhost:3000"
else
    print_error "Vite dev server is NOT running on localhost:3000"
    print_info "Start the dev server with: cd web && npm run dev"
    exit 1
fi

# Test 3: Test Vite proxy for /auth route
print_header "Test 3: Vite Proxy - /auth Route"
echo "Testing: http://localhost:3000/auth/login"
echo ""

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:3000/auth/login \
    -H "Content-Type: application/json" \
    -d '{"login":"admin","password":"admin"}' 2>&1)

if [ "$HTTP_CODE" = "200" ]; then
    print_success "Proxy working! Got 200 response"
    print_info "Login response body omitted because it contains credentials"
elif [ "$HTTP_CODE" = "401" ]; then
    print_info "Proxy working but credentials invalid (401)"
    print_info "This means the proxy is working, just need correct credentials"
else
    print_error "Proxy test failed with HTTP $HTTP_CODE"
    print_info "Login response body omitted because it may contain credentials"
fi

# Test 4: Test direct API access with CORS
print_header "Test 4: Direct API Access with CORS Headers"
echo "Testing: http://localhost:8080/auth/login with Origin header"
echo ""

CORS_RESPONSE=$(curl -s -D - -o /dev/null -X POST http://localhost:8080/auth/login \
    -H "Content-Type: application/json" \
    -H "Origin: http://localhost:3000" \
    -d '{"login":"admin","password":"admin"}' 2>&1)

if echo "$CORS_RESPONSE" | grep -q "Access-Control-Allow-Origin"; then
    print_success "CORS headers present in response"
    echo "$CORS_RESPONSE" | grep -i "access-control"
else
    print_error "No CORS headers found in response"
    print_info "This might cause issues if bypassing the proxy"
fi

# Test 5: CORS Preflight (OPTIONS request)
print_header "Test 5: CORS Preflight Request"
echo "Testing OPTIONS request to http://localhost:8080/auth/login"
echo ""

OPTIONS_RESPONSE=$(curl -s -i -X OPTIONS http://localhost:8080/auth/login \
    -H "Origin: http://localhost:3000" \
    -H "Access-Control-Request-Method: POST" \
    -H "Access-Control-Request-Headers: Content-Type" 2>&1)

if echo "$OPTIONS_RESPONSE" | grep -q "Access-Control-Allow-Origin: http://localhost:3000"; then
    print_success "CORS preflight successful for localhost:3000"
elif echo "$OPTIONS_RESPONSE" | grep -q "Access-Control-Allow-Origin"; then
    ALLOWED_ORIGIN=$(echo "$OPTIONS_RESPONSE" | grep "Access-Control-Allow-Origin" | cut -d: -f2- | tr -d ' \r')
    print_error "CORS configured for different origin: $ALLOWED_ORIGIN"
else
    print_error "CORS preflight failed - no Access-Control-Allow-Origin header"
fi

echo -e "\nFull preflight response headers:"
echo "$OPTIONS_RESPONSE" | grep -i "access-control" || echo "No CORS headers found"

# Test 6: Check browser environment variables
print_header "Test 6: Environment Configuration"

if [ -f "web/.env" ]; then
    print_info "Found web/.env (contents omitted)"
else
    print_success "No web/.env file (using defaults)"
fi

if [ -f "web/.env.local" ]; then
    print_info "Found web/.env.local (contents omitted)"
else
    print_success "No web/.env.local file"
fi

# Test 7: Check api-config.ts
print_header "Test 7: Frontend API Configuration"

if [ -f "web/src/lib/api-config.ts" ]; then
    echo "Current api-config.ts BASE URL setting:"
    grep -A 1 "API_BASE_URL" web/src/lib/api-config.ts || print_error "Could not find API_BASE_URL"

    echo -e "\nWITH_CREDENTIALS setting:"
    grep "WITH_CREDENTIALS" web/src/lib/api-config.ts || print_error "Could not find WITH_CREDENTIALS"
else
    print_error "web/src/lib/api-config.ts not found!"
fi

# Test 8: Check Vite config
print_header "Test 8: Vite Proxy Configuration"

if [ -f "web/vite.config.ts" ]; then
    echo "Proxy configuration in vite.config.ts:"
    grep -A 10 "proxy:" web/vite.config.ts | head -n 15 || print_error "Could not find proxy config"
else
    print_error "web/vite.config.ts not found!"
fi

# Summary
print_header "Summary & Recommendations"

echo "1. API Server: $(curl -s -f http://localhost:8080/health > /dev/null 2>&1 && echo -e "${GREEN}Running${NC}" || echo -e "${RED}Not Running${NC}")"
echo "2. Vite Dev Server: $(curl -s -f http://localhost:3000 > /dev/null 2>&1 && echo -e "${GREEN}Running${NC}" || echo -e "${RED}Not Running${NC}")"
echo ""

print_info "Recommended Development Setup:"
echo "   1. Start API: ./scripts/start_services_test.sh"
echo "   2. Start Frontend: cd web && npm run dev"
echo "   3. Access UI at: http://localhost:3000"
echo "   4. All requests will be proxied automatically"
echo ""

print_info "Checklist for CORS Issues:"
echo "   [ ] API server running on localhost:8080"
echo "   [ ] Vite dev server running on localhost:3000"
echo "   [ ] OpenAPI.BASE = \"\" in web/src/lib/api-config.ts"
echo "   [ ] OpenAPI.WITH_CREDENTIALS = true"
echo "   [ ] Vite proxy configured for /api and /auth"
echo "   [ ] No VITE_API_BASE_URL environment variable set"
echo "   [ ] Browser DevTools shows requests to localhost:3000 (not 8080)"
echo ""

print_info "If still having CORS errors:"
echo "   1. Restart Vite dev server (changes require restart)"
echo "   2. Clear browser cache and localStorage"
echo "   3. Check browser console for actual request URL"
echo "   4. Review web/CORS-TROUBLESHOOTING.md for detailed guide"
