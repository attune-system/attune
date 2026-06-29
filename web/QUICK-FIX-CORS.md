# Quick Fix: CORS Error on Login

## TL;DR - Do This First

**The frontend needs to be restarted to pick up configuration changes:**

```bash
# Stop the Vite dev server (Ctrl+C in the terminal where it's running)
# Then restart it:
cd web
npm run dev
```

**After restart:**

1. Hard refresh your browser: `Ctrl+Shift+R` (Linux/Windows) or `Cmd+Shift+R` (Mac)
2. Or clear browser cache and reload
3. Try logging in with: `admin` / `admin`

## Why This Fixes It

We made two important changes that require a restart:

1. **Changed API base URL** from `http://localhost:8080` to `""` (empty string)
   - This makes requests go through the Vite proxy
   - No more CORS errors!

2. **Added `/auth` proxy route** to `vite.config.ts`
   - Vite now proxies both `/api/*` and `/auth/*` routes

3. **Fixed the login form** to submit on Enter key
   - Changed button from `type="button"` to `type="submit"`

## Verify It's Working

After restarting and refreshing:

1. **Open Browser DevTools** (F12)
2. **Go to Network tab**
3. **Try to login**
4. **Check the request URL** - it should be:
   - ✅ `http://localhost:3000/auth/login` (CORRECT - uses proxy)
   - ❌ `http://localhost:8080/auth/login` (WRONG - direct to API)

If you see `localhost:3000`, the proxy is working and CORS won't be an issue!

## Debug Output

We added console logging to `api-config.ts`. After restart, you should see in browser console:

```
🔧 API Configuration:
  - VITE_API_BASE_URL env: undefined
  - Resolved BASE URL:
  - WITH_CREDENTIALS: true
  - This means requests will be: RELATIVE (using proxy)
```

If you see `ABSOLUTE to http://localhost:8080`, something is wrong - check if you have a `.env` file setting `VITE_API_BASE_URL`.

## Still Not Working?

### Check 1: Is Vite Dev Server Actually Restarted?

The old process might still be running. Kill it completely:

```bash
# Find and kill any running Vite processes
pkill -f vite
# Or if using tmux
tmux kill-session -t <your-session-name>

# Then start fresh
cd web
npm run dev
```

### Check 2: Clear Browser Storage

```bash
# In browser console (F12):
localStorage.clear()
sessionStorage.clear()
location.reload()
```

### Check 3: Verify Configuration Files

**File: `web/src/lib/api-config.ts`**
Should have:

```typescript
const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || "";
OpenAPI.BASE = API_BASE_URL;
OpenAPI.WITH_CREDENTIALS = true;
```

**File: `web/vite.config.ts`**
Should have:

```typescript
proxy: {
  "/api": {
    target: "http://localhost:8080",
    changeOrigin: true,
  },
  "/auth": {
    target: "http://localhost:8080",
    changeOrigin: true,
  },
}
```

### Check 4: No Environment Variables

Make sure you don't have these set:

```bash
# Should NOT be set:
VITE_API_BASE_URL=http://localhost:8080  # ❌ Remove this!
```

Check for these files and remove/rename them:

- `web/.env`
- `web/.env.local`
- `web/.env.development`

## Test Credentials

- **Username:** `admin`
- **Password:** `admin`

If you need to reset the user again:

```bash
./scripts/create_test_user.sh
```

## Enter Key Now Works!

After the fix, you can:

1. Type username
2. Press Tab
3. Type password
4. Press Enter ✨

No need to click the button anymore!

## Run Full Diagnostics

If you're still stuck, run the diagnostic script:

```bash
./web/scripts/test-cors.sh
```

This will check:

- Is API server running?
- Is Vite dev server running?
- Is proxy working?
- Configuration files correct?

## Architecture Reminder

```
Browser → http://localhost:3000 (Vite) → Proxy → http://localhost:8080 (API)
          ↑
          Same origin - No CORS!
```

The browser only talks to `localhost:3000`. Vite forwards requests to the API behind the scenes.

## For More Help

See the detailed guide: `web/CORS-TROUBLESHOOTING.md`
