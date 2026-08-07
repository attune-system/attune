# Git Pack Installation - Quick Reference

**Quick commands and examples for installing packs from git repositories**

---

## Installation Methods

### Web UI
```
Packs → Add Pack ▼ → Install from Remote → Git Repository
```

### CLI
```bash
attune pack install <git-url> [--ref <branch|tag|commit>] [options]
```

### API
```bash
POST /api/v1/packs/install
```

---

## Quick Examples

### Public GitHub Repository
```bash
# Latest from default branch
attune pack install https://github.com/example/pack-slack.git

# Specific version tag
attune pack install https://github.com/example/pack-slack.git --ref v2.1.0

# Specific branch
attune pack install https://github.com/example/pack-slack.git --ref develop

# Specific commit
attune pack install https://github.com/example/pack-slack.git --ref a1b2c3d
```

### Installation Options
```bash
# Force reinstall (replace existing)
attune pack install <url> --force

# Skip tests
attune pack install <url> --skip-tests

# Skip dependency validation
attune pack install <url> --skip-deps

# All options combined
attune pack install <url> --ref v1.0.0 --force --skip-tests --skip-deps
```

---

## Git URL Formats

### HTTPS
```
✓ https://github.com/username/pack-name.git
✓ https://gitlab.com/username/pack-name.git
✓ https://bitbucket.org/username/pack-name.git
✓ https://git.example.com/username/pack-name.git
```

SSH, `git://`, `file://`, and credential-bearing URLs are rejected. Distribute
private packs as archives through an authenticated registry.

---

## Git References

| Type | Example | Description |
|------|---------|-------------|
| Tag | `v1.2.3` | Semantic version tag |
| Tag | `release-2024-01-27` | Release tag |
| Branch | `main` | Main branch |
| Branch | `develop` | Development branch |
| Branch | `feature/xyz` | Feature branch |
| Commit | `a1b2c3d4e5f6...` | Full commit hash |
| Commit | `a1b2c3d` | Short commit hash (7+ chars) |
| None | (omit --ref) | Default branch (shallow) |

---

## Installation Flags

| Flag | Effect | Use When |
|------|--------|----------|
| `--force` | Replace existing pack, bypass checks | Upgrading, testing |
| `--skip-tests` | Don't run pack tests | Tests slow/unavailable |
| `--skip-deps` | Don't validate dependencies | Custom environment |

⚠️ **Warning**: Use flags cautiously in production!

---

## Required Pack Structure

```
repository/
├── pack.yaml          ← Required
├── actions/           ← Optional
├── sensors/           ← Optional
├── caches/            ← Optional
└── ...
```

OR

```
repository/
└── pack/
    ├── pack.yaml      ← Required
    └── ...
```

---

## API Request

```bash
curl -X POST http://localhost:8080/api/v1/packs/install \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "source": "https://github.com/example/pack-slack.git",
    "ref_spec": "v2.1.0",
    "force": false,
    "skip_tests": false,
    "skip_deps": false
  }'
```

---

## Common Workflows

### Production Install
```bash
# Install specific stable version
attune pack install https://github.com/myorg/pack-prod.git --ref v1.0.0
```

### Development Testing
```bash
# Install from feature branch, skip checks
attune pack install https://github.com/myorg/pack-dev.git \
  --ref feature/new-action \
  --force \
  --skip-tests
```

### CI/CD Pipeline
```bash
# Install from current commit
attune pack install https://github.com/$REPO.git \
  --ref $COMMIT_SHA \
  --force
```

---

## Troubleshooting

| Error | Solution |
|-------|----------|
| Permission denied | Use an anonymously readable HTTPS repository or an authenticated registry archive |
| Ref not found | Verify branch/tag exists and is pushed |
| pack.yaml not found | Ensure file exists at root or in pack/ |
| Dependencies missing | Install dependencies or use --skip-deps |
| Tests failed | Fix tests or use --skip-tests or --force |

---

## Security Best Practices

✓ **DO**:
- Use specific tags in production (`v1.2.3`)
- Use authenticated registry archives for private repos
- Review code before installing
- Rotate access tokens regularly

✗ **DON'T**:
- Embed credentials in URLs
- Install from `main` branch in production
- Skip validation without review
- Use force mode carelessly

---

## Web UI Workflow

1. Navigate to **Packs** page
2. Click **Add Pack** dropdown button
3. Select **Install from Remote**
4. Choose **Git Repository** source type
5. Enter repository URL
6. (Optional) Enter git reference
7. (Optional) Configure installation options
8. Click **Install Pack**
9. Wait for completion and redirect

---

## Quick Tips

💡 **Version Control**: Always use tags for production (e.g., `v1.0.0`)

💡 **Testing**: Test from feature branch first, then install from tag

💡 **Shallow Clone**: Omit ref for faster install (default branch only)

💡 **Commit Hash**: Most specific reference, guaranteed reproducibility

---

## Related Commands

```bash
# List installed packs
attune pack list

# View pack details
attune pack show <pack-ref>

# Run pack tests
attune pack test <pack-ref>

# Uninstall pack
attune pack uninstall <pack-ref>
```

---

## More Information

📖 Full documentation: `docs/packs/pack-installation-git.md`
📖 Pack structure: `docs/packs/pack-structure.md`
📖 Pack registry spec: `docs/packs/pack-registry-spec.md`
