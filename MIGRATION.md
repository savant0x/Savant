# Migration Guide

Retrofit the ECHO Protocol into an existing project.

## Quick Setup

### 1. Copy Protocol Files

```bash
# From your project root
git clone https://github.com/savant0x/Savant.git /tmp/echo-protocol
cp -r /tmp/echo-protocol/ECHO.md .
cp -r /tmp/echo-protocol/protocol.config.yaml .
cp -r /tmp/echo-protocol/STARTER-PROMPT.md .
cp -r /tmp/echo-protocol/MIGRATION.md .
cp -r /tmp/echo-protocol/VERSION .
cp -r /tmp/echo-protocol/CHANGELOG.md .
cp -r /tmp/echo-protocol/LICENSE .
cp -r /tmp/echo-protocol/README.md .
cp -r /tmp/echo-protocol/.markdownlint.json .
cp -r /tmp/echo-protocol/overview.jpg .
cp -r /tmp/echo-protocol/templates/ ./templates/
cp -r /tmp/echo-protocol/coding-standards/ ./coding-standards/
```

### 2. Scaffold Runtime Directories

```bash
mkdir -p dev/fids/archive dev/session-summaries
```

### 3. Create .gitignore Entries

Add these lines to your `.gitignore`:

```text
# ECHO Protocol runtime state
dev/session-summaries/*
dev/fids/*
!dev/fids/.gitkeep
!dev/fids/archive/
!dev/fids/archive/.gitkeep
!.gitkeep

# Keep LEARNINGS.md tracked
!dev/LEARNINGS.md
```

### 4. Create LEARNINGS.md

```bash
cat > dev/LEARNINGS.md << 'EOF'
# LEARNINGS

<!-- Add new entries above this line -->
EOF
```

### 5. Configure protocol.config.yaml

Edit `protocol.config.yaml` and set:

```yaml
protocol:
  version: "0.1.0"
  strict_mode: true

project:
  name: "your-project-name"
  description: "Your project description"
  version: "your-project-version"

language: "rust" # rust | typescript | python | go | java | csharp

commands:
  build: "your-build-command"
  test: "your-test-command"
  type_check: "your-type-check-command"
  lint: "your-lint-command"
  format: "your-format-command"
  clean: "your-clean-command"
```

### 6. Verify Setup

Your project root should now contain:

```text
your-project/
├── ECHO.md
├── protocol.config.yaml       # Configured with your language + commands
├── STARTER-PROMPT.md
├── MIGRATION.md               # This file
├── README.md                  # Landing page
├── VERSION
├── CHANGELOG.md
├── LICENSE
├── .markdownlint.json
├── overview.jpg                 # Protocol overview diagram
├── templates/
│   ├── FID-TEMPLATE.md
│   └── SESSION-SUMMARY.md
├── coding-standards/
│   └── {your-language}.md
├── dev/
│   ├── LEARNINGS.md
│   ├── fids/
│   │   └── archive/
│   └── session-summaries/
└── [your existing project files]
```

### 7. Activate the Protocol

Copy the appropriate starter prompt from `STARTER-PROMPT.md` into your
AI agent's system prompt. The agent will boot, prove compliance, and
begin working under full protocol discipline.

---

## Retrofit Checklist

Use this checklist when adding the protocol to an existing project:

- [ ] All protocol files copied to project root
- [ ] `dev/fids/` and `dev/session-summaries/` directories created
- [ ] `.gitignore` updated with protocol runtime paths
- [ ] `dev/LEARNINGS.md` created
- [ ] `protocol.config.yaml` configured with correct language
- [ ] `protocol.config.yaml` configured with correct build/test commands
- [ ] `protocol.config.yaml` project name and version set
- [ ] Starter prompt copied to agent system prompt
- [ ] Boot sequence verified (agent confirms all laws and config)

---

## Adding to Multiple Projects

The protocol is designed to be forked into each project independently.
Each project gets its own `protocol.config.yaml` with project-specific
commands and settings, but shares the same underlying discipline.

### Recommended Approach

1. Maintain a "golden copy" of the protocol (this repo)
2. When starting a new project, copy the golden copy into the project
3. Configure `protocol.config.yaml` for that project's language/toolchain
4. Each project evolves independently — customize as needed

### Updating Existing Projects

When the protocol gets a new version:

1. Check the [CHANGELOG](CHANGELOG.md) for breaking changes
2. Copy updated protocol files into your project
3. Preserve your `protocol.config.yaml` (don't overwrite project-specific settings)
4. Preserve your `dev/` directory (contains your session history)
5. Update `VERSION` to match the new protocol version

### Namespace Convention for Existing Projects

If your project already has its own `CHANGELOG.md`, `README.md`, or
`coding-standards/` directory, protocol files will collide with yours at
the project root. Use a `protocol/` subdirectory to keep them clean:

```text
your-project/
├── protocol/                    # All protocol files live here
│   ├── ECHO.md
│   ├── protocol.config.yaml
│   ├── STARTER-PROMPT.md
│   ├── MIGRATION.md
│   ├── VERSION
│   ├── CHANGELOG.md             # Protocol changelog (not yours)
│   ├── LICENSE
│   ├── .markdownlint.json
│   ├── overview.jpg
│   ├── templates/
│   ├── coding-standards/
│   └── dev/
├── CHANGELOG.md                 # Your project's changelog
├── README.md                    # Your project's readme
├── coding-standards/            # Your project's standards (if any)
└── [your existing project files]
```

When using this layout, update `protocol.config.yaml` paths to point
to `protocol/dev/` instead of `dev/`. The starter prompt should reference
`protocol/ECHO.md` and `protocol/protocol.config.yaml`.

> **Recommendation:** Use the `protocol/` namespace if your project already
> has a CHANGELOG.md, README.md, or coding-standards/ at the root. This
> prevents file collisions and keeps protocol updates clean — you can
> overwrite the entire `protocol/` directory without touching your project files.
