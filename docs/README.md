# Attune Documentation

Welcome to the Attune project documentation! This directory contains comprehensive documentation for the Attune automation and orchestration platform.

## 📚 Quick Navigation

### Getting Started
- **[Quick Start Guide](guides/quick-start.md)** - Get up and running quickly
- **[Workspace Setup](development/WORKSPACE_SETUP.md)** - Set up your development environment
- **[Configuration Guide](configuration/configuration.md)** - Configure Attune services

### Core Concepts
- **[Architecture Overview](architecture/)** - System design and service architecture
- **[Workflow Engine](workflows/workflow-execution-engine.md)** - How workflows are executed
- **[Pack System](packs/pack-structure.md)** - Understanding automation packs
- **[Sensors & Triggers](sensors/)** - Event monitoring and triggering

### Development
- **[API Documentation](api/)** - REST API endpoint reference
- **[CLI Reference](cli/cli.md)** - Command-line interface guide
- **[Testing Guide](testing/running-tests.md)** - How to run and write tests
- **[Authentication](authentication/authentication.md)** - Auth mechanisms and security

### Operations
- [PostgreSQL-only deployment](deployment/postgresql-only.md) describes fresh installation, database volume selection, and separate existing-data conversion or reset decisions.
- **[Production Deployment](deployment/production-deployment.md)** - Deploy to production
- **[Supervisor Service](deployment/supervisor.md)** - Runtime retention, maintenance jobs, corrective actions, and supervisor configuration
- **[Operational Visibility](deployment/operational-visibility.md)** - Worker cordon, health, alerts, execution reconciliation, and sensor logs
- **[Runtime Log Verification](deployment/runtime-log-verification.md)** - Cross-replica integration checks and bounded load reports
- **[Operations Runbook](deployment/ops-runbook-queues.md)** - Troubleshooting and maintenance
- **[Performance Optimization](performance/QUICKREF-performance-optimization.md)** - Performance tuning

## 📁 Directory Structure

```
docs/
├── api/                  # REST API documentation
├── architecture/         # System architecture and service design
├── authentication/       # Auth, security, and secrets management
├── cli/                  # Command-line interface documentation
├── configuration/        # Configuration guides and troubleshooting
├── dependencies/         # Dependency management documentation
├── deployment/           # Production deployment and operations
├── development/          # Developer guides and tooling
├── examples/             # Example configurations and workflows
├── guides/               # Getting started guides and tutorials
├── migrations/           # Database migration documentation
├── packs/                # Pack system documentation
├── performance/          # Performance optimization guides
├── plans/                # Future planning and proposals
├── sensors/              # Sensor system documentation
├── testing/              # Testing strategies and guides
├── web-ui/               # Web UI documentation
├── webhooks/             # Webhook system documentation
└── workflows/            # Workflow engine documentation
```

## 🔍 Finding What You Need

### By Role

**I'm a new developer:**
1. Start with [Workspace Setup](development/WORKSPACE_SETUP.md)
2. Read the [Quick Start Guide](guides/quick-start.md)
3. Review [Architecture Overview](architecture/)
4. Learn about [Testing](testing/running-tests.md)

**I'm building a pack:**
1. Read [Pack Structure](packs/pack-structure.md)
2. Check [Examples](examples/)
3. Review [Pack Testing Framework](packs/pack-testing-framework.md)
4. See [Core Pack Integration](packs/core-pack-integration.md)

**I'm creating workflows:**
1. Start with [Workflow Quickstart](guides/workflow-quickstart.md)
2. Learn [Workflow Orchestration](workflows/workflow-orchestration.md)
3. Understand [Rule Parameter Mapping](workflows/rule-parameter-mapping.md)
4. Review [Execution Hierarchy](workflows/execution-hierarchy.md)

**I'm deploying to production:**
1. Read [Production Deployment](deployment/production-deployment.md)
2. Review [Configuration Guide](configuration/configuration.md)
3. Check [Security Review](authentication/security-review-2024-01-02.md)
4. Study [Operations Runbook](deployment/ops-runbook-queues.md)

**I'm using the API:**
1. Browse [API Documentation](api/)
2. Check [OpenAPI Spec](api/openapi-spec-completion.md)
3. Learn [Authentication](authentication/auth-quick-reference.md)
4. Review endpoint-specific guides (e.g., [Actions](api/api-actions.md), [Work Queues](api/api-work-queues.md))

### By Topic

| Topic | Primary Location | Key Files |
|-------|-----------------|-----------|
| Authentication | `authentication/` | `authentication.md`, `auth-quick-reference.md` |
| Workflows | `workflows/` | `workflow-execution-engine.md`, `workflow-orchestration.md` |
| Sensors | `sensors/` | `sensor-interface.md`, `sensor-lifecycle-management.md` |
| Testing | `testing/` | `running-tests.md`, `schema-per-test.md` |
| Performance | `performance/` | `QUICKREF-performance-optimization.md` |
| Configuration | `configuration/` | `configuration.md`, `config-troubleshooting.md` |
| CLI | `cli/` | `cli.md`, `cli-profiles.md` |

## 🔎 Search Tips

**Search by content:**
```bash
# Find all docs mentioning "execution"
grep -r "execution" docs/

# Search within a specific area
grep -r "webhook" docs/webhooks/
```

**Search by filename:**
```bash
# Find all architecture docs
find docs/architecture/ -name "*.md"

# Find testing guides
find docs/testing/ -name "*.md"
```

**Use the AGENTS.md index:**
```bash
# View minified documentation index
cat AGENTS.md

# Regenerate after adding new docs
make generate-agents-index
```

## 📖 Documentation Conventions

### File Naming
- Use lowercase with hyphens: `my-feature-guide.md`
- Prefix with category when helpful: `api-actions.md`, `testing-authentication.md`
- Use descriptive names: `workflow-execution-engine.md` not `wf-exec.md`

### Content Structure
Most documentation files follow this structure:
1. **Overview** - What this document covers
2. **Key Concepts** - Important terminology and concepts
3. **How-To** - Step-by-step instructions
4. **Examples** - Code/config examples
5. **Troubleshooting** - Common issues and solutions
6. **References** - Links to related docs

### Cross-Referencing
Use relative links to reference other docs:
```markdown
See [Authentication Guide](../authentication/authentication.md) for details.
```

## 🤝 Contributing Documentation

### Adding New Docs
1. Choose the appropriate subdirectory
2. Follow naming conventions
3. Include frontmatter if using a doc generator
4. Add cross-references to related docs
5. Run `make generate-agents-index` to update the index

### Updating Existing Docs
1. Keep the overall structure intact
2. Add new sections at the end if needed
3. Update the table of contents if present
4. Check for broken links

### Work Summaries
Daily work and session notes go in `work-summary/`, not here:
- `work-summary/sessions/` - Daily work logs
- `work-summary/status/` - Status updates
- `work-summary/phases/` - Phase completion summaries

## 🔗 External Resources

- **GitHub Repository**: [attune](https://github.com/yourusername/attune)
- **Issue Tracker**: GitHub Issues
- **Discussions**: GitHub Discussions

## 📝 Documentation Organization

For details on the documentation organization system, see [Documentation Organization](development/documentation-organization.md).

This explains:
- Why each directory exists
- What belongs where
- How to maintain the structure
- Reorganization history

## 💡 Tips

- **Start broad, go deep**: Begin with guides and overviews, then dive into specific topics
- **Check examples**: The `examples/` directory has working configurations
- **Use grep**: Full-text search is your friend for finding specific topics
- **Update AGENTS.md**: After adding docs, regenerate with `make generate-agents-index`

---

*Last updated: 2026-01-30*
*Documentation files: 100+*
*Categories: 16*
