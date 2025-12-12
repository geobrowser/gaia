# Search Indexer Repository Architecture

## Sequence Diagram

```mermaid
sequenceDiagram
    participant App as Application Code
    participant Service as SearchIndexService
    participant Provider as SearchIndexProvider (Trait)
    participant OpenSearch as OpenSearchProvider
    participant Backend as OpenSearch Cluster

    App->>Service: update(UpdateEntityRequest)
    Service->>Service: validate_uuid(entity_id)
    Service->>Service: validate_uuid(space_id)
    Service->>Provider: update_document(request)
    Provider->>OpenSearch: update_document(request)
    OpenSearch->>Backend: HTTP via opensearch crate (/index/_update/{id} with doc_as_upsert)
    Backend-->>OpenSearch: Response (200 OK)
    OpenSearch-->>Provider: Ok(())
    Provider-->>Service: Ok(())
    Service-->>App: Ok(())

    Note over App,Backend: Error Flow (Validation Error)
    App->>Service: update(invalid_request)
    Service->>Service: validate_uuid() fails
    Service-->>App: Err(SearchIndexError::ValidationError)

    Note over App,Backend: Error Flow (Backend Error)
    App->>Service: update(request)
    Service->>Provider: update_document(request)
    Provider->>OpenSearch: update_document(request)
    OpenSearch->>Backend: HTTP via opensearch crate (/index/_update/{id})
    Backend-->>OpenSearch: Response (500 Error)
    OpenSearch-->>Provider: Err(SearchIndexError::UpdateError)
    Provider-->>Service: Err(SearchIndexError::UpdateError)
    Service-->>App: Err(SearchIndexError::UpdateError)
```

## Layered Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        Application Code                         │
│  - Uses SearchIndexService for all operations                   │
│  - Handles SearchIndexError                                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                    SearchIndexService                           │
│  - Validates input (UUIDs, batch sizes)                         │
│  - Converts requests to EntityDocument                          │
│  - Delegates operations to SearchIndexProvider                  │
│  - Returns SearchIndexError                                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                SearchIndexProvider (Trait)                      │
│  - Abstract backend interface                                   │
│  - Methods: update_document (upsert), delete_document          │
│    + bulk_update, bulk_delete                                  │
│  - Returns SearchIndexError                                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             │ Implementation
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                  OpenSearchProvider                             │
│  - Implements SearchIndexProvider                               │
│  - Makes calls to an OpenSearch cluster using the opensearch    │
│    Rust crate for all REST calls                                │
│  - Handles index configuration and OpenSearch-specific logic    │
│  - Converts errors from opensearch crate to SearchIndexError    │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                   OpenSearch Cluster                            │
│  - OpenSearch server                                            │
│  - Stores and indexes documents                                 │
│  - Returns HTTP responses                                       │
└─────────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

### SearchIndexService
- **Input validation**: UUID format, required fields, batch size limits
- **Request handling**: UpdateEntityRequest (upsert: creates or updates)
- **Error handling**: All errors are SearchIndexError
- **Configuration**: Batch size limits, etc.

### SearchIndexProvider (Trait)
- **Abstract interface**: Defines contract for all backend implementations
- **Operation methods**: CRUD and bulk operations
- **Error type**: Returns SearchIndexError for all operations

### OpenSearchProvider
- **Implements SearchIndexProvider**: Concrete backend implementation
- **HTTP communication**: All calls to OpenSearch cluster are performed using the [opensearch Rust crate](https://docs.rs/opensearch/)
- **Error conversion**: Translates OpenSearch errors into SearchIndexError
- **Index management**: Handles index creation, aliases, etc.

## Error Flow

All errors propagate as `SearchIndexError` through each layer:

```
OpenSearch Cluster Error
    ↓
OpenSearchProvider (converts to SearchIndexError)
    ↓
SearchIndexProvider (passes through)
    ↓
SearchIndexService (passes through)
    ↓
Application Code (handles SearchIndexError)
```

## Example Data Flow: Updating/Creating a Document

```
1. Application: update(UpdateEntityRequest { entity_id: "123", ... })
   ↓
2. SearchIndexService: Validates UUIDs
   ↓
3. SearchIndexProvider: update_document(&UpdateEntityRequest)
   ↓
4. OpenSearchProvider: Makes HTTP update request with doc_as_upsert=true
   ↓
5. OpenSearch: Creates document if missing, updates if exists, returns 200 OK
   ↓
6. Response flows back: Ok(()) → Application
```

