//! The OpenAPI document, written by hand.
//!
//! Hand-written rather than generated from the handlers, and that is a deliberate trade. A
//! generated document is guaranteed to match the code and is guaranteed to be unreadable: it
//! names every extractor, every `Json<Value>` becomes an untyped object, and the descriptions a
//! client actually needs — *why* a run is `409`, what an empty transcript search means — are
//! nowhere, because they are in prose in this crate and not in a type.
//!
//! So the document is a literal, and the thing that keeps it honest is the API test that checks
//! every path the router registers appears in it. A document that drifts from the code is worse
//! than no document, because a client believes it.

use serde_json::{json, Value};

/// The paths this API serves, in the order the document lists them.
///
/// Kept beside the document so a test can compare the two lists against the router's own.
pub const PATHS: &[&str] = &[
    "/v1/health",
    "/v1/capabilities",
    "/v1/projects",
    "/v1/projects/{id}",
    "/v1/projects/{id}/sources",
    "/v1/projects/{id}/segments",
    "/v1/projects/{id}/segments/{segment_id}",
    "/v1/projects/{id}/preview",
    "/v1/projects/{id}/run",
    "/v1/runs/{run_id}",
    "/v1/runs/{run_id}/cancel",
    "/v1/transcripts/search",
    "/v1/openapi.json",
];

/// The OpenAPI 3.1 document.
#[must_use]
pub fn openapi_document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "TheTrimmer local daemon",
            "version": env!("CARGO_PKG_VERSION"),
            "description":
                "A local control surface over the project store, the cut queue and the \
                 verifier. It binds to 127.0.0.1 only: it can cut files and delete projects, so \
                 it is not a service and must not be exposed to a network. Every request needs \
                 `Authorization: Bearer <token>`.",
            "license": { "name": "LicenseRef-Proprietary" }
        },
        "servers": [ { "url": "http://127.0.0.1:{port}", "variables": {
            "port": { "default": "8787" }
        }}],
        "components": {
            "securitySchemes": {
                "bearer": { "type": "http", "scheme": "bearer" }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error", "detail"],
                    "properties": {
                        "error": { "type": "string", "description": "A short word a client can switch on." },
                        "detail": { "type": "string", "description": "A sentence a person can read." }
                    }
                },
                "Project": {
                    "type": "object",
                    "description": "The project as the readable document form, which is the same shape `trimmer-store`'s document module writes.",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "name": { "type": "string" },
                        "createdBy": { "type": "string" },
                        "createdAt": { "type": "integer" },
                        "updatedAt": { "type": "integer" },
                        "defaultPreset": { "type": "string" },
                        "verify": { "type": "string", "enum": ["off", "standard", "strict", "forensic"] },
                        "outputDir": { "type": ["string", "null"] },
                        "sources": { "type": "object" },
                        "segments": { "type": "array" },
                        "presets": { "type": "object" }
                    }
                },
                "Segment": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "source": { "type": "string" },
                        "name": { "type": "string" },
                        "startFrame": { "type": "integer" },
                        "endFrame": { "type": ["integer", "null"], "description": "One past the last frame kept, or null to the end of the source." },
                        "note": { "type": ["string", "null"] },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "preset": { "type": ["string", "null"] },
                        "handleFrames": { "type": "integer" },
                        "enabled": { "type": "boolean" }
                    }
                },
                "Run": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "projectId": { "type": "string", "format": "uuid" },
                        "state": { "type": "string", "enum": ["queued", "running", "finished", "cancelled"] },
                        "startedAt": { "type": "integer" },
                        "finishedAt": { "type": ["integer", "null"] },
                        "total": { "type": "integer" },
                        "outcome": { "type": ["object", "null"], "description": "Present once the run has finished." },
                        "error": { "type": ["string", "null"] }
                    }
                }
            }
        },
        "security": [ { "bearer": [] } ],
        "paths": {
            "/v1/health": {
                "get": {
                    "summary": "Liveness, and whether the machine can actually cut.",
                    "operationId": "health",
                    "responses": {
                        "200": { "description": "The daemon is up.", "content": { "application/json": { "schema": {
                            "type": "object",
                            "properties": {
                                "status": { "type": "string" },
                                "service": { "type": "string" },
                                "version": { "type": "string" },
                                "ffmpeg": { "type": "boolean" },
                                "ffmpeg": { "type": "boolean" }
                            }
                        }}}},
                        "401": { "description": "No token, or the wrong one.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/capabilities": {
                "get": {
                    "summary": "What this machine's ffmpeg can do.",
                    "operationId": "capabilities",
                    "responses": {
                        "200": { "description": "The capability report.", "content": { "application/json": { "schema": { "type": "object" } } } },
                        "401": { "description": "No token, or the wrong one.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects": {
                "get": {
                    "summary": "Every project in the store, newest first.",
                    "operationId": "listProjects",
                    "responses": {
                        "200": { "description": "The projects.", "content": { "application/json": { "schema": { "type": "array", "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "format": "uuid" },
                                "name": { "type": "string" },
                                "updatedAt": { "type": "integer" }
                            }
                        }}}}},
                        "401": { "description": "No token, or the wrong one.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                },
                "post": {
                    "summary": "Create a project.",
                    "operationId": "createProject",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["name", "created_by"],
                        "properties": {
                            "name": { "type": "string" },
                            "created_by": { "type": "string" }
                        }
                    }}}},
                    "responses": {
                        "201": { "description": "The project that was created.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Project" } } } },
                        "400": { "description": "The body is malformed.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } },
                        "401": { "description": "No token, or the wrong one.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}": {
                "get": {
                    "summary": "One project.",
                    "operationId": "getProject",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "The project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Project" } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                },
                "delete": {
                    "summary": "Delete a project, and everything it owns.",
                    "operationId": "deleteProject",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "It is gone.", "content": { "application/json": { "schema": { "type": "object" } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}/sources": {
                "post": {
                    "summary": "Add a source, probing it when it is on disk.",
                    "description": "A path that is not there is still recorded, marked unavailable: a project has to survive a drive being unplugged.",
                    "operationId": "addSource",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "requestBody": { "required": true, "content": { "application/json": { "schema": {
                        "type": "object", "required": ["path"], "properties": { "path": { "type": "string" } }
                    }}}},
                    "responses": {
                        "201": { "description": "The source as it was recorded.", "content": { "application/json": { "schema": { "type": "object" } } } },
                        "400": { "description": "The body is malformed.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}/segments": {
                "get": {
                    "summary": "The project's segments, in running order.",
                    "operationId": "listSegments",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "The segments.", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Segment" } } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                },
                "post": {
                    "summary": "Mark a cut.",
                    "description": "`end_frame` is one past the last frame kept, or null for the end of the source. A range that is not a range is refused with 400.",
                    "operationId": "addSegment",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "requestBody": { "required": true, "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["source", "name", "start_frame"],
                        "properties": {
                            "source": { "type": "string" },
                            "name": { "type": "string" },
                            "start_frame": { "type": "integer" },
                            "end_frame": { "type": ["integer", "null"] },
                            "preset": { "type": ["string", "null"] },
                            "handle_frames": { "type": "integer" }
                        }
                    }}}},
                    "responses": {
                        "201": { "description": "The segment that was created.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Segment" } } } },
                        "400": { "description": "The body is malformed, the source is not in the project, or the range is impossible.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}/segments/{segment_id}": {
                "delete": {
                    "summary": "Remove a mark.",
                    "operationId": "deleteSegment",
                    "parameters": [
                        { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
                        { "name": "segment_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
                    ],
                    "responses": {
                        "200": { "description": "It is gone.", "content": { "application/json": { "schema": { "type": "object" } } } },
                        "404": { "description": "There is no such project or segment.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}/preview": {
                "get": {
                    "summary": "The dry run: what a batch would do, and the exact commands it would run.",
                    "description": "Nothing is written. The commands come from the same argument builders the executor uses, so a preview cannot drift from what will actually run.",
                    "operationId": "previewProject",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "One entry per enabled segment.", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/projects/{id}/run": {
                "post": {
                    "summary": "Start a batch and return its run id.",
                    "description": "The batch runs on a task; poll `GET /v1/runs/{run_id}` for its state. A second run for the same project is 409.",
                    "operationId": "startRun",
                    "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "202": { "description": "Accepted.", "content": { "application/json": { "schema": {
                            "type": "object",
                            "properties": {
                                "runId": { "type": "string", "format": "uuid" },
                                "state": { "type": "string" }
                            }
                        }}}},
                        "404": { "description": "There is no such project.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } },
                        "409": { "description": "A run for this project is already going, or the registry is full of runs in flight.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/runs/{run_id}": {
                "get": {
                    "summary": "A run's state, and its outcome once it has finished.",
                    "operationId": "getRun",
                    "parameters": [ { "name": "run_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "The run.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Run" } } } },
                        "404": { "description": "There is no such run, or it has been evicted from the bounded registry.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/runs/{run_id}/cancel": {
                "post": {
                    "summary": "Ask a running batch to stop.",
                    "description": "The segment in flight finishes; the ones after it are not attempted. A half-written segment is worse than one that was never started.",
                    "operationId": "cancelRun",
                    "parameters": [ { "name": "run_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ],
                    "responses": {
                        "200": { "description": "Cancellation was requested.", "content": { "application/json": { "schema": { "type": "object" } } } },
                        "404": { "description": "There is no such run, or it is not running.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/transcripts/search": {
                "post": {
                    "summary": "Search the caption file beside a video.",
                    "description": "A video with no caption file answers 200 with an empty list and a `reason`, not 404: there being no transcript is a fact about the file rather than a missing resource.",
                    "operationId": "searchTranscripts",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": {
                        "type": "object",
                        "required": ["video", "phrase"],
                        "properties": {
                            "video": { "type": "string" },
                            "phrase": { "type": "string" },
                            "limit": { "type": "integer", "default": 20 }
                        }
                    }}}},
                    "responses": {
                        "200": { "description": "The hits, possibly empty.", "content": { "application/json": { "schema": {
                            "type": "object",
                            "properties": {
                                "video": { "type": "string" },
                                "phrase": { "type": "string" },
                                "rate": { "type": "string" },
                                "hits": { "type": "array", "items": { "type": "object" } },
                                "reason": { "type": "string" }
                            }
                        }}}},
                        "400": { "description": "The body is malformed.", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                }
            },
            "/v1/openapi.json": {
                "get": {
                    "summary": "This document.",
                    "operationId": "openapi",
                    "responses": { "200": { "description": "The OpenAPI document.", "content": { "application/json": { "schema": { "type": "object" } } } } }
                }
            }
        }
    })
}
