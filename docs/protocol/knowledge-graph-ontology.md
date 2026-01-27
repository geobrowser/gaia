# Knowledge Graph Ontology Spec

**Status:** Draft **Version:** 0.1.0

## 1. Types, Properties, and Schemas

The knowledge graph is schema-less by default. Instead, schemas are represented as entities in-graph and act as conventions or hints for how data should be modeled. These typed representations of knowledge are often called "Ontologies."

### 1.1 Schemas as Entities

All schema components are entities. Entities can have Types, and Types are themselves entities that have the Type: `Type`. Type membership is expressed as a relation whose relation type id is the Types property `8f151ba4de204e3c9cb499ddf96f48f1`.

Types can define a schema (e.g. Name, Description, Avatar, Birthdate). Each property in a schema is itself an entity with Type: `Property` (`808a04ceb21c4d888ad12e240613e5ca`).

```
[Person] --TYPES--> [Type]
[Type]   --PROPERTY--> [Name]
[Name]   --TYPES--> [Property]
```

### 1.2 Property Metadata

Each property may optionally define:

- A **Data Type** (as in `api/src/services/storage/schema.ts`)
- A **Renderable Type** (e.g. URL)

Depending on the data type or renderable type, a property may include extra metadata such as **Format** or **Unit**. These are modeled as properties themselves.

```
[Name] --DATA_TYPE--> [Text]
[Website] --DATA_TYPE--> [TEXT]
[Website] --RENDERABLE_TYPE--> [URL]
[Measurement] --UNIT--> [Kilogram]
[Timestamp] --FORMAT--> [ISO_8601]
```

## 2. Renderable Types

Renderable types are UI representations of underlying data types or entities. They are hints for clients on how to display the underlying data model. For example, a URL is stored as TEXT but can be rendered with specialized UI. Videos, Images, Places, Addresses, and similar entities may also have specific presentation patterns.

### 2.1 Known Renderable Types

| Renderable Type | UUID | Description |
|---|---|---|
| Time interval | `ba71f735d8e444f79535ea98981fde22` | Render time spans or moments. |
| Video | `0fb6bbf022044db49f70fa82c41570a4` | Render linked video media. |
| Image | `f3f790c4c74e4d23a0a91e8ef84e30d9` | Render linked image media. |
| Place | `edc4b62157e94ccc9f60f38903edb720` | Render a place entity with location context. |
| URL | `283127c96142468492ed90b0ebc7f29a` | Render a URL with link-specific UI. |
| Address | `e95864bfde0f4453914a0ab67ec41ad2` | Render a postal or physical address. |
| Geo location | `9cf5c1b015dc451cbfd297db64806aff` | Render a geographic point or area. |

## 3. Blocks (as Relations)

Blocks are rich content for an entity. Each block is itself an entity, and blocks are attached to a parent via the Blocks relation. Because blocks are entities, they can be attached to multiple parents, which enables transclusion. The relation type id for Blocks is the Blocks property entity: `beaba5cba67741a8b35377030613fc70`.

### 3.1 Block Ordering

Blocks are ordered using the `position` field on the Blocks relation. Positions are fractional-index position-strings over the alphabet `0-9A-Za-z`, ordered lexicographically, with a maximum length of 64 characters (see [GRC-20 spec](https://github.com/geobrowser/grc-20/blob/main/spec.md)).

### 3.2 Block Types

| Block Type | UUID |
|---|---|
| Text Block | `76474f2f00894e77a0410b39fb17d0bf` |
| Data Block | `b8803a8665de412bbb357e0c84adf473` |
| Image | `ba4e41460010499da0a3caaa7f579d0e` |

### 3.3 Block Example

```
[Page] --BLOCKS{position=a}--> [Text Block]
[Page] --BLOCKS{position=aV}--> [Data Block]
[Page] --BLOCKS{position=b}--> [Image]
```

### 3.4 Text Block Requirements

| Property | UUID | Description | Target |
|---|---|---|---|
| Markdown content | `e3e363d1dd294ccb8e6ff3b76d99bc33` | Markdown body for the text block. | TEXT value |

### 3.5 Data Block Requirements

| Property | UUID | Description | Target |
|---|---|---|---|
| Data source type | `1f69cc9880d444abad493df6a7b15ee4` | Declares whether the data source is a query or a collection. | Query data source `3b069b04adbe4728917d1283fd4ac27e` or Collection data source `1295037a5d9c4d09b27c5502654b9177` |
| Filters | `14a46854bfd14b1882152785c2dab9f3` | JSON-encoded filters applied to the data source. | JSON value (filter spec TBD) |
| Collection item | `a99f9ce12ffa4dac8c61f6310d46064a` | Entity included in a collection data source. | Any entity |

Data source types:
- Query data source `3b069b04adbe4728917d1283fd4ac27e`
- Collection data source `1295037a5d9c4d09b27c5502654b9177`

Query data block: uses the Query data source type and defines a declarative graph query that is evaluated live. The query is stored on the block using the Filters property as JSON-encoded query data (spec pending).

Collection data block: uses the Collection data source type and enumerates a fixed, ordered set of entities via Collection item relations. Ordering is expressed via the position on each Collection item relation. Filters can also be applied to collections using the Filters property.

### 3.6 Data Block Views

Data block view types are defined on the relation pointing to the block using the View property `1907fd1c81114a3ca378b1f353425b65`.

| View Type | UUID |
|---|---|
| Gallery view | `ccb70fc917f04a54b86e3b4d20cc7130` |
| List view | `7d497dba09c249b8968f716bcf520473` |
| Bulleted list view | `0aaac6f7c916403eaf6d2e086dc92ada` |
| Table view | `cba271cef7c140339047614d174c69f1` |

### 3.7 Image Requirements

| Property | UUID | Description | Target |
|---|---|---|---|
| URL | `8a743832c0944a62b6650c3cc2f9c7bc` | Source URL for the image. | TEXT value |

## 4. Images (Entities + Relations)

Images are entities. The Image entity type is `ba4e41460010499da0a3caaa7f579d0e` and uses the same URL property as image blocks.

### 4.1 Image Properties

| Property | UUID | Description | Target |
|---|---|---|---|
| URL | `8a743832c0944a62b6650c3cc2f9c7bc` | Source URL for the image. | TEXT value |
| Width | `f7b33e08b76d4190aadacadaa9f561e1` | Image width. | FLOAT64 value |
| Height | `7f6ad0433e214257a6d48bdad36b1d84` | Image height. | FLOAT64 value |

### 4.2 File Type Relation

Images can specify a file type using the File type relation `515f346fe0fb40c78ea95339787eecc1`, which points to a file type entity (not yet specified).

## 5. Topics and Representing a Space

Spaces can set a topic that represents what the space is about and is used to determine the space’s front page. The topic is an arbitrary entity in the knowledge graph; there is no canonical Topic type or UUID yet.

Setting the topic is done onchain via the `SET_TOPIC` action in the protocol, not via a knowledge-graph relation.

## 6. System Properties

### 6.1 System Entity Registry

All IDs defined in this spec are system entities.

| Name | UUID | Notes |
|---|---|---|
| Type | `e7d737c536764c609fa16aa64a8c90ad` | Type entity used to denote schemas (i.e., Types: Type). |
| Property | `808a04ceb21c4d888ad12e240613e5ca` | Type entity used to mark property definitions. |
| Types | `8f151ba4de204e3c9cb499ddf96f48f1` | Relation type id for type membership. |
| Properties | `01412f8381894ab1836565c7fd358cc1` | Relation used to attach properties to a schema/type. |
| Name | `a126ca530c8e48d5b88882c734c38935` | Human-readable name for the entity. |
| Description | `9b1f76ff9711404c861e59dc3fa7d037` | Short description used in previews and summaries. |
| Cover | `34f535072e6b42c5a84443981a77cfa2` | Banner-style image for the entity. |
| Blocks | `beaba5cba67741a8b35377030613fc70` | Rich content blocks attached to the entity. |
| Time interval | `ba71f735d8e444f79535ea98981fde22` | Renderable type. |
| Video | `0fb6bbf022044db49f70fa82c41570a4` | Renderable type. |
| Image (renderable) | `f3f790c4c74e4d23a0a91e8ef84e30d9` | Renderable type. |
| Place | `edc4b62157e94ccc9f60f38903edb720` | Renderable type. |
| URL (renderable) | `283127c96142468492ed90b0ebc7f29a` | Renderable type. |
| Address | `e95864bfde0f4453914a0ab67ec41ad2` | Renderable type. |
| Geo location | `9cf5c1b015dc451cbfd297db64806aff` | Renderable type. |
| Data source type | `1f69cc9880d444abad493df6a7b15ee4` | Declares whether a data block is query-based or collection-based. |
| Query data source | `3b069b04adbe4728917d1283fd4ac27e` | Marker entity for live, declarative queries. |
| Collection data source | `1295037a5d9c4d09b27c5502654b9177` | Marker entity for fixed, enumerated entity sets. |
| Filters | `14a46854bfd14b1882152785c2dab9f3` | JSON-encoded query/filter data (spec TBD). |
| Collection item | `a99f9ce12ffa4dac8c61f6310d46064a` | Points to an entity in a collection. |
| Markdown content | `e3e363d1dd294ccb8e6ff3b76d99bc33` | Markdown body for a text block. |
| Text Block | `76474f2f00894e77a0410b39fb17d0bf` | Block entity containing markdown content. |
| Data Block | `b8803a8665de412bbb357e0c84adf473` | Block entity that renders query/collection results. |
| Image | `ba4e41460010499da0a3caaa7f579d0e` | Image entity for media with URL and dimensions. |
| URL | `8a743832c0944a62b6650c3cc2f9c7bc` | Source URL for an image. |
| Width | `f7b33e08b76d4190aadacadaa9f561e1` | Image width. |
| Height | `7f6ad0433e214257a6d48bdad36b1d84` | Image height. |
| View | `1907fd1c81114a3ca378b1f353425b65` | Sets the preferred rendering mode for a data block relation. |
| Gallery view | `ccb70fc917f04a54b86e3b4d20cc7130` | Render results as a gallery/grid. |
| List view | `7d497dba09c249b8968f716bcf520473` | Render results as a list. |
| Bulleted list view | `0aaac6f7c916403eaf6d2e086dc92ada` | Render results as a bulleted list. |
| Table view | `cba271cef7c140339047614d174c69f1` | Render results as a table. |
| File type | `515f346fe0fb40c78ea95339787eecc1` | Points to a file type entity (not yet specified). |
