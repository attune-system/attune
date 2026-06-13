# Quick Start: Vite Dev Server for Local Development

**Fast iteration on the Attune Web UI with hot-module reloading!**

## TL;DR

```bash
# Terminal 1: Start backend services (once)
docker compose up -d postgres rabbitmq redis api executor worker-shell sensor

# Terminal 2: Start Vite dev server (restart as needed)
cd web
npm install  # First time only
npm run dev

# Browser: Open http://localhost:3001
```

## Common Commands

### Start Development Environment

```bash
# Start all required backend services
docker compose up -d postgres rabbitmq redis api executor worker-shell sensor

# Start Vite dev server
cd web && npm run dev
```

### Stop Development Environment

```bash
# Stop Vite (in terminal running npm run dev)
Ctrl+C

# Stop backend services
docker compose stop

# Or completely remove containers
docker compose down
```

### Restart API After Code Changes

```bash
# Rebuild and restart API service
docker compose up -d --build api

# Vite dev server keeps running - no restart needed!
```

### View Logs

```bash
# View API logs
docker compose logs -f api

# View all services
docker compose logs -f

# View specific service
docker compose logs -f executor
```

### Troubleshooting

```bash
# Health check API
curl http://localhost:8080/health

# Check CORS configuration
docker compose logs api | grep -i cors

# List running containers
docker compose ps

# Restart a service
docker compose restart api

# Clear Vite cache
rm -rf web/node_modules/.vite
```

## Default Ports

| Service | Port | URL |
|---------|------|-----|
| **Vite Dev Server** | 3001 | http://localhost:3001 |
| API Service | 8080 | http://localhost:8080 |
| PostgreSQL | 5432 | postgresql://localhost:5432 |
| RabbitMQ | 5672 | amqp://localhost:5672 |
| RabbitMQ Management | 15672 | http://localhost:15672 |
| Valkey / Redis metadata cache | 6379 | redis://localhost:6379 |
| Notifier WebSocket | 8081 | ws://localhost:8081 |
| Docker Web (NGINX) | 3000 | http://localhost:3000 |

## Why Port 3001?

The Docker web container (NGINX) uses port 3000. Vite dev server is configured to use 3001 to avoid conflicts. This gives you:

- ✅ Hot-module reloading (HMR) for fast development
- ✅ Instant feedback on code changes
- ✅ Full access to Docker backend services
- ✅ No CORS issues

## Testing Login

Default test user (created by Docker init):

- **Email**: `test@attune.local`
- **Password**: `TestPass123!`

## Common Issues

### CORS Errors

**Fix:** Restart the API service after any config changes:
```bash
docker compose restart api
```

### Port 3001 Already in Use

**Fix:** Vite will automatically try 3002, 3003, etc. Or manually specify:
```bash
npm run dev -- --port 3005
```

### API Not Responding

**Fix:** Check if API is running:
```bash
docker compose ps api
curl http://localhost:8080/health
```

If not running:
```bash
docker compose up -d api
```

### Database Connection Failed

**Fix:** Make sure PostgreSQL is running and initialized:
```bash
docker compose up -d postgres
docker compose logs postgres

# Wait for migrations to complete
docker compose logs migrations
```

## Development Workflow

### Morning Routine

```bash
# Start all backend services
docker compose up -d postgres rabbitmq redis api executor worker-shell sensor

# Start frontend dev server
cd web && npm run dev
```

### During Development

- Edit React/TypeScript files in `web/src/`
- Changes appear instantly in browser (no reload!)
- API changes require rebuilding the API container

### End of Day

```bash
# Stop Vite dev server
Ctrl+C (in terminal running npm run dev)

# Optional: Stop backend services to free resources
docker compose stop

# Or keep them running for faster start tomorrow!
```

## What's Running Where?

```
┌─────────────────────────────────────────────┐
│           Your Local Machine                │
│                                             │
│  Browser ←→ Vite Dev (3001)                │
│                ↓                            │
│             Proxy                           │
│                ↓                            │
│  ┌──────────────────────────────────────┐  │
│  │  Docker Containers                   │  │
│  │  - API (8080)                        │  │
│  │  - PostgreSQL (5432)                 │  │
│  │  - RabbitMQ (5672)                   │  │
│  │  - Redis (6379)                      │  │
│  │  - Workers, Executor, Sensor         │  │
│  └──────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

## Next Steps

- **Full documentation**: See [vite-dev-setup.md](./vite-dev-setup.md)
- **API endpoints**: http://localhost:8080/docs (Swagger UI)
- **Architecture docs**: See `docs/architecture/`

## Pro Tips

1. **Keep backend running** between sessions - saves startup time
2. **Use HMR effectively** - most changes don't need page reload
3. **Test production build** before committing:
   ```bash
   cd web && npm run build && npm run preview
   ```
4. **Monitor API logs** while developing to catch backend errors:
   ```bash
   docker compose logs -f api
   ```

## Help!

If something isn't working:

1. Check service health: `docker compose ps`
2. View logs: `docker compose logs -f`
3. Restart everything:
   ```bash
   docker compose down
   docker compose up -d postgres rabbitmq redis api executor worker-shell sensor
   cd web && npm run dev
   ```
4. Check the [full documentation](./vite-dev-setup.md)

Happy coding! 🚀