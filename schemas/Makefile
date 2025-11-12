# Schema validation and compilation
.PHONY: validate compile sync all watch

# Validate all protobuf schemas
validate:
	@echo "Validating protobuf schemas..."
	@for file in proto/*.proto; do \
		protoc --descriptor_set_out=/dev/null --include_imports $$file || exit 1; \
	done
	@echo "✓ All schemas valid"

# Compile schemas to multiple languages
compile:
	@echo "Compiling schemas..."
	@echo "Note: Rust uses build.rs with prost for compile-time generation"
	@echo "To regenerate Rust code, run: cd ../producer && cargo build"
	# TypeScript (if needed)
	# protoc --ts_out=../consumer-ts/src/generated proto/*.proto
	@echo "✓ Schema compilation configured"

# Sync to k8s ConfigMap
sync: validate
	@echo "Syncing to Kubernetes ConfigMap..."
	@kubectl create configmap protobuf-schemas \
		--from-file=proto/ \
		--dry-run=client \
		-o yaml > ../k8s/protobuf-configmap.yaml
	@echo "✓ ConfigMap updated"

# Run all
all: validate compile sync

# Watch for changes
watch:
	@echo "Watching for schema changes..."
	@fswatch -o proto/*.proto | xargs -n1 -I{} make all
