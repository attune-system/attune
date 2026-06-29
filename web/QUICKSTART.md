# Attune Web UI - Quick Start Guide

Get the Attune Web UI running in 5 minutes.

## Prerequisites

- Node.js 18+ installed
- Attune API service running (see main README)

## Quick Start

```bash
# 1. Install dependencies
cd web
npm install

# 2. Start development server
npm run dev
```

The UI will be available at **http://localhost:3000**

## First Login

The web UI connects to the API service at `http://localhost:8080` by default.

1. Open http://localhost:3000
2. You'll be redirected to the login page
3. Enter credentials from your Attune installation
4. After login, you'll see the dashboard

### Default Test Credentials

If using the development database with seed data:

- **Username**: `admin`
- **Password**: (depends on your setup)

## Configuration

The web UI is configured via environment variables:

```env
VITE_API_BASE_URL=http://localhost:8080  # API service URL
VITE_WS_URL=ws://localhost:8081          # WebSocket URL (future)
```

To customize, create `.env.development.local`:

```bash
cp .env.development .env.development.local
# Edit values as needed
```

## Common Tasks

### Build for Production

```bash
npm run build
```

Output will be in `dist/` directory.

### Preview Production Build

```bash
npm run preview
```

### Generate API Client

When the API changes, regenerate the client:

```bash
npm run generate:api
```

This requires the API service to be running.

## Troubleshooting

### "Connection refused" errors

Ensure the API service is running:

```bash
cd ..
make dev  # or your preferred method to start services
```

### Login fails

Check that:

1. API service is running on port 8080
2. Database is migrated
3. User exists in the database

### Build errors

Clear cache and rebuild:

```bash
rm -rf node_modules/.tmp
npm run build
```

## Development

- **Hot Reload**: Changes to files automatically reload
- **TypeScript**: Full type checking during development
- **Linting**: Run `npm run lint` to check code

## Next Steps

- Read the full [README.md](./README.md) for detailed documentation
- Review [Web UI Architecture](../docs/web-ui-architecture.md)
- Check [TODO](../work-summary/TODO.md) for planned features

## Need Help?

- Check logs in browser DevTools Console
- Review API responses in Network tab
- See [Troubleshooting](./README.md#troubleshooting) in README
