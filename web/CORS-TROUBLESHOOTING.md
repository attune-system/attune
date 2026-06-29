# CORS Troubleshooting Guide

This guide explains how CORS is configured in Attune and how to troubleshoot common issues.

## Architecture Overview

Attune uses a **proxy-based architecture** for local development:

```
Browser (localhost:3000) → Vite Dev Server → Proxy → API Server (localhost:8080)
```

### Why Use a Proxy?

1. **Avoids CORS issues** - Requests appear to come from the same origin
2. **Simpler configuration** - No need to configure CORS for every local development scenario
3. **Production-ready** - In production, use a reverse proxy (nginx/caddy) the same way

## Current Configuration

### Frontend (Vite Proxy)

**File:** `web/vite.config.ts`

```typescript
server: {
  port: 3000,
  proxy: {
    "/api": {
      target: "http://localhost:8080",
      changeOrigin: true,
    },
    "/auth": {
      target: "http://localhost:8080",
      changeOrigin: true,
    },
  },
}
```

**What this does:**

- Requests to `http://localhost:3000/api/*` → `http://localhost:8080/api/*`
- Requests to `http://localhost:3000/auth/*` → `http://localhost:8080/auth/*`

### Frontend API Client

**File:** `web/src/lib/api-config.ts`

```typescript
const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || "";
OpenAPI.BASE = API_BASE_URL;
OpenAPI.WITH_CREDENTIALS = true;
OpenAPI.CREDENTIALS = "include";
```

**Key settings:**

- `BASE = ""` - Makes requests relative (uses proxy)
- `WITH_CREDENTIALS = true` - Sends cookies/auth headers
- `CREDENTIALS = "include"` - Include credentials in cross-origin requests

### Backend CORS Configuration

**File:** `crates/api/src/middleware/cors.rs`

**Default allowed origins** (when no custom origins configured):

```
http://localhost:3000
http://localhost:5173
http://localhost:8080
http://127.0.0.1:3000
http://127.0.0.1:5173
http://127.0.0.1:8080
```

**Allowed methods:** GET, POST, PUT, DELETE, PATCH, OPTIONS

**Allowed headers:** Authorization, Content-Type, Accept

**Credentials:** Enabled (`allow_credentials(true)`)

## Common Issues & Solutions

### Issue 1: "CORS policy: No 'Access-Control-Allow-Origin' header"

**Cause:** Frontend making direct requests to `http://localhost:8080` instead of using proxy

**Solution:**

```typescript
// ❌ Wrong - bypasses proxy
const response = await fetch('http://localhost:8080/auth/login', ...);

// ✅ Correct - uses proxy
const response = await fetch('/auth/login', ...);
```

**Check:**

1. Verify `OpenAPI.BASE = ""` in `web/src/lib/api-config.ts`
2. Don't set `VITE_API_BASE_URL` environment variable locally

### Issue 2: "CORS policy: credentials mode is 'include'"

**Cause:** `WITH_CREDENTIALS` mismatch between client and server

**Solution:**

1. Ensure `OpenAPI.WITH_CREDENTIALS = true` (frontend)
2. Ensure CORS layer has `.allow_credentials(true)` (backend)
3. Cannot use `allow_origin(Any)` with credentials - must specify origins

### Issue 3: OPTIONS preflight request failing

**Cause:** Browser sends OPTIONS request before actual request, server doesn't handle it

**Solution:**

- Axum's `CorsLayer` automatically handles OPTIONS requests
- Verify `Method::OPTIONS` is in `.allow_methods()`
- Check server logs for OPTIONS requests

### Issue 4: Custom frontend port not working

**Cause:** Your dev server runs on a different port (e.g., 5173 instead of 3000)

**Solution:**

**Option A:** Update Vite config to use port 3000:

```typescript
server: {
  port: 3000,
  // ...
}
```

**Option B:** Add your port to backend CORS origins:

```yaml
# config.development.yaml
server:
  cors_origins:
    - "http://localhost:5173"
    - "http://localhost:3000"
```

Or set environment variable:

```bash
export ATTUNE__SERVER__CORS_ORIGINS='["http://localhost:5173"]'
```

### Issue 5: Production deployment CORS errors

**Cause:** Production frontend domain not in allowed origins

**Solution:**

1. **Set production CORS origins:**

```yaml
# config.production.yaml
server:
  cors_origins:
    - "https://app.example.com"
    - "https://www.example.com"
```

2. **Use environment variables:**

```bash
export ATTUNE__SERVER__CORS_ORIGINS='["https://app.example.com"]'
```

3. **Or use a reverse proxy** (recommended):
   - Frontend and backend served from same domain
   - No CORS needed!

## Testing CORS Configuration

### Test 1: Verify Proxy is Working

```bash
# Start dev server
cd web
npm run dev

# In another terminal, test proxy
curl -v http://localhost:3000/auth/login
```

Should show request proxied to backend.

### Test 2: Check Allowed Origins

```bash
# Test CORS preflight
curl -X OPTIONS http://localhost:8080/auth/login \
  -H "Origin: http://localhost:3000" \
  -H "Access-Control-Request-Method: POST" \
  -v
```

Look for these headers in response:

```
Access-Control-Allow-Origin: http://localhost:3000
Access-Control-Allow-Credentials: true
Access-Control-Allow-Methods: GET, POST, PUT, DELETE, PATCH, OPTIONS
```

### Test 3: Actual Login Request

```bash
curl -X POST http://localhost:8080/auth/login \
  -H "Origin: http://localhost:3000" \
  -H "Content-Type: application/json" \
  -d '{"login":"admin","password":"admin"}' \
  -v
```

Check for `Access-Control-Allow-Origin` in response.

## Development Workflow

### Recommended Setup

1. **Start backend services:**

```bash
./scripts/start_services_test.sh
```

2. **Start frontend dev server:**

```bash
cd web
npm run dev
```

3. **Access UI:**
   - Open browser to `http://localhost:3000`
   - All API requests automatically proxied
   - No CORS issues!

### Alternative: Direct API Access

If you need to access API directly (e.g., testing with curl/Postman):

1. Set environment variable:

```bash
export ATTUNE__SERVER__CORS_ORIGINS='["http://localhost:3000","*"]'
```

2. Restart API service

**⚠️ Warning:** Never use `"*"` in production!

## Environment Variables Reference

```bash
# Allow all origins (DEVELOPMENT ONLY!)
export ATTUNE__SERVER__CORS_ORIGINS='["*"]'

# Allow specific origins
export ATTUNE__SERVER__CORS_ORIGINS='["http://localhost:3000","http://localhost:5173"]'

# Frontend: Use direct API access (bypasses proxy)
export VITE_API_BASE_URL='http://localhost:8080'

# Frontend: Use proxy (default, recommended)
# Don't set VITE_API_BASE_URL, or set to empty string
export VITE_API_BASE_URL=''
```

## Quick Fix Checklist

If you're getting CORS errors, check these in order:

- [ ] Frontend dev server running on `localhost:3000`?
- [ ] `OpenAPI.BASE = ""` in `web/src/lib/api-config.ts`?
- [ ] Vite proxy configured for `/api` and `/auth`?
- [ ] Backend API running on `localhost:8080`?
- [ ] Not setting `VITE_API_BASE_URL` environment variable?
- [ ] Browser console shows requests to `localhost:3000`, not `8080`?

## Additional Resources

- [MDN: CORS](https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS)
- [Vite Proxy Config](https://vitejs.dev/config/server-options.html#server-proxy)
- [Tower HTTP CORS](https://docs.rs/tower-http/latest/tower_http/cors/)

## Summary

**For local development:**

- Use Vite proxy (default configuration)
- Set `OpenAPI.BASE = ""`
- Frontend requests go to `localhost:3000`, proxied to `8080`

**For production:**

- Use reverse proxy (nginx/caddy/traefik)
- OR configure explicit CORS origins in `config.production.yaml`
- Never use wildcard `*` origins with credentials
