# Schema Management

This directory contains the source of truth for all protobuf schemas used in the Kafka infrastructure.

## Philosophy

**Schemas are code, not configuration.** They should be:
- ✅ Version controlled
- ✅ Validated at compile time
- ✅ Tested for compatibility
- ✅ Generated to multiple languages
- ✅ Synced to runtime environments

## Directory Structure

```
hermes/
├── schemas/           # Central schema definitions (this directory)
│   ├── proto/         # Source protobuf schemas
│   │   └── user_event.proto
│   ├── Makefile       # Build, validate, and sync
│   └── README.md      # This file
├── producer/          # Rust producer (depends on schemas/)
├── k8s/               # Kubernetes configs (depends on schemas/)
└── ...
```

**Architecture Benefits:**
- ✅ **Schemas are independent** - Not coupled to any specific service or infrastructure
- ✅ **Clean dependencies** - Producer and k8s both depend on schemas, not each other
- ✅ **Reusability** - Multiple consumers (TypeScript, Python, Go) can reference the same schemas
- ✅ **Single source of truth** - One location for all schema definitions

## Workflow

### 1. Edit Schema

Edit schemas in `proto/` directory:

```bash
vim proto/user_event.proto
```

### 2. Validate

Check schemas are valid:

```bash
make validate
```

### 3. Regenerate Rust Code

After editing proto files, regenerate the Rust code:

1. **Uncomment the build.rs code:**
   ```bash
   # In hermes-schema/build.rs, uncomment the prost-build code
   ```

2. **Build the crate:**
   ```bash
   cargo build -p hermes-schema
   ```

3. **Comment build.rs back:**
   ```bash
   # Re-comment the prost-build code to avoid rebuilding on every compile
   ```

4. **Commit the generated files:**
   The generated Rust files in `src/pb/` should be committed to version control.

**Why comment/uncomment?** The build.rs is commented out to avoid regenerating protos on every build. Proto generation only needs to happen when `.proto` files change.

### 4. Sync to Kubernetes

Update the k8s ConfigMap:

```bash
make sync
```

This regenerates `k8s/protobuf-configmap.yaml`.

### 5. Deploy

```bash
cd ../k8s
kubectl apply -f protobuf-configmap.yaml
```

### All at Once

```bash
make all
```

## Type Safety Benefits

### Before (Manual ConfigMap)
```yaml
# k8s/protobuf-configmap.yaml
data:
  user_event.proto: |
    syntax = "proto3";  # Typos not caught until runtime
    messag UserEvent {  # ❌ Syntax error, no validation
```

### After (Generated from Source)
```bash
make sync
# ✅ Validates syntax
# ✅ Checks for compatibility
# ✅ Generates type-safe code
# ✅ Auto-syncs to ConfigMap
```

## Schema Evolution

### Adding Fields (Safe)

```protobuf
message UserEvent {
  string event_id = 1;
  string user_id = 2;
  string new_field = 3;  // ✅ Safe - backward compatible
}
```

### Removing Fields (Careful)

```protobuf
message UserEvent {
  string event_id = 1;
  // string user_id = 2;  // ⚠️  Mark deprecated instead
  reserved 2;              // ✅ Reserve field number
  reserved "user_id";      // ✅ Reserve field name
}
```

### Changing Types (Breaking)

```protobuf
message UserEvent {
  string event_id = 1;
  int64 user_id = 2;  // ❌ BREAKING - was string
}
```

## Compatibility Testing

Add tests to verify backward/forward compatibility:

```bash
# TODO: Add buf or protovalidate for automated checks
buf breaking --against .git#branch=main
```

## CI/CD Integration

Add to your CI pipeline:

```yaml
# .github/workflows/schemas.yml
name: Validate Schemas
on: [push, pull_request]
jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Validate schemas
        run: cd schemas && make validate
      - name: Check generated code is up to date
        run: |
          cd schemas
          make compile sync
          git diff --exit-code
```

## Best Practices

1. **Never edit ConfigMap directly** - Always edit source `.proto` files
2. **Run `make validate` before committing** - Catch errors early
3. **Version your schemas** - Use semantic versioning
4. **Document breaking changes** - In commit messages and CHANGELOG
5. **Test compatibility** - Before deploying to production

## Tools

### Install protoc

```bash
# macOS
brew install protobuf

# Linux
apt-get install -y protobuf-compiler
```

### Install Rust plugin

```bash
cargo install protobuf-codegen
```

### Install buf (optional, for better validation)

```bash
# macOS
brew install bufbuild/buf/buf

# Linux
curl -sSL "https://github.com/bufbuild/buf/releases/download/v1.28.1/buf-Linux-x86_64" \
  -o /usr/local/bin/buf
chmod +x /usr/local/bin/buf
```

## Migration from Current Setup

The current setup has schemas defined inline in the ConfigMap. To migrate:

1. **Copy schemas to this directory** - ✅ Done
2. **Validate they compile** - `make validate`
3. **Regenerate ConfigMap** - `make sync`
4. **Update producer to use generated code** - Already done
5. **Deploy updated ConfigMap** - `kubectl apply -f ../protobuf-configmap.yaml`

## Alternatives Considered

### Option 1: Schema Registry (Confluent)
- ✅ Centralized schema management
- ✅ Automatic compatibility checking
- ❌ Extra infrastructure to run
- ❌ Additional cost

### Option 2: Buf Schema Registry
- ✅ Modern tooling
- ✅ Good CI/CD integration
- ❌ Requires buf.build account

### Option 3: Git as source of truth (Current approach)
- ✅ Simple, no extra infrastructure
- ✅ Version controlled
- ✅ Works with existing tools
- ⚠️  Manual sync to k8s required

## Future Improvements

- [ ] Add automated compatibility testing
- [ ] Generate TypeScript types for web consumers
- [ ] Add schema versioning
- [ ] Integrate with Buf Schema Registry
- [ ] Add pre-commit hooks for validation
