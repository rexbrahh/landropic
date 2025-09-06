# Git Repository Divergence Diagnosis & Resolution Method

## Quick Diagnosis Protocol

### Step 1: Initial State Assessment
Run these commands in parallel to get the complete picture:

```bash
git status                    # Working tree state
git branch -vv               # Local branches with tracking info
git remote -v                # Remote repository URLs
```

### Step 2: Remote Information Gathering
```bash
git fetch --all --prune      # Get latest remote state, clean stale references
git log --oneline --graph --decorate --all -10  # Visual history comparison
```

### Step 3: Divergence Analysis
```bash
# Count commits ahead/behind
git rev-list --count --left-right main...origin/main

# Show commit history divergence visually
git log --oneline --graph --decorate main origin/main -10

# Compare local vs remote commits
git log --oneline main -10
git log --oneline origin/main -10

# See actual content differences
git diff main origin/main
```

### Step 4: Identify Common Issues

**Issue Indicators:**
- `warning: refname 'main' is ambiguous` → Multiple refs with same name
- Branch shows no upstream tracking (`git branch -vv` shows no `[origin/main]`)
- `fatal: 'origin' does not appear to be a git repository` → Non-standard remote names
- Large commit count differences (e.g., "23	21") → Significant divergence

**Remote Name Detection:**
```bash
git branch -r  # List remote branches to identify actual remote name
```

## Resolution Strategies

### Strategy A: Keep All Local Changes (Force Push)
**Use when:** You want to discard remote history entirely

```bash
# If using standard 'origin' remote:
git push --force-with-lease origin main

# If using non-standard remote name (e.g., 'main'):
git push --force-with-lease main main

# Set up proper tracking:
git branch --set-upstream-to=origin/main main  # or main/main for non-standard remotes
```

### Strategy B: Merge Both Histories
**Use when:** You want to preserve both local and remote changes

```bash
git pull --no-ff origin main  # Creates merge commit
# Resolve conflicts manually if they occur
git add .
git commit -m "Merge remote changes"
```

### Strategy C: Reset to Remote
**Use when:** You want to discard local changes and match remote

```bash
git checkout -b backup-local     # Save current work
git checkout main
git reset --hard origin/main     # Match remote exactly
```

## Common Troubleshooting

### Non-Standard Remote Names
If you see `main/main` instead of `origin/main`:
- Your remote is named "main" instead of "origin"
- Use `main` as the remote name in commands
- Example: `git push main main` instead of `git push origin main`

### Missing Upstream Configuration
```bash
# Fix with:
git branch --set-upstream-to=REMOTE/BRANCH LOCAL_BRANCH
# Example: git branch --set-upstream-to=main/main main
```

### Verification After Resolution
```bash
git status                    # Should show "up to date"
git log --oneline -5         # Verify expected commits
git branch -vv               # Confirm tracking is set up
```

## Safety Considerations

1. **Always use `--force-with-lease`** instead of `--force` when force-pushing
2. **Create backup branches** before destructive operations
3. **Verify remote state** with `git fetch` before major operations
4. **Check for uncommitted changes** with `git status` first
5. **Communicate with team** before force-pushing to shared repositories

## Decision Matrix

| Scenario | Recommended Action | Risk Level |
|----------|-------------------|------------|
| Local has all needed work, remote is outdated | Force push with lease | Low |
| Both have valuable changes | Merge | Medium |
| Remote is authoritative | Reset to remote | Medium |
| Unsure of what remote contains | Backup local, then investigate | Low |

## Example Session Commands

```bash
# Complete diagnosis in one go:
git status && git fetch --all --prune && git log --oneline --graph --all -10

# Quick divergence check:
git rev-list --count --left-right main...origin/main

# Safe force push (most common resolution):
git push --force-with-lease origin main
git branch --set-upstream-to=origin/main main
```

This method prioritizes **safety** and **information gathering** before making any destructive changes to repository history.