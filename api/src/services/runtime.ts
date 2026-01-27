/**
 * Application runtime configuration.
 *
 * Provides a ManagedRuntime initialized once at startup with all layers,
 * avoiding per-request layer building overhead.
 */

import {Layer, ManagedRuntime} from "effect"

import {Environment, make as makeEnvironment} from "./environment"
import {make as makeStorage, Storage} from "./storage/storage"
import {TelemetryLive} from "./telemetry"

// Layer definitions
const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))

// Combined layer with all dependencies including telemetry
const AppLayer = Layer.mergeAll(EnvironmentLayer, StorageLayer, TelemetryLive)

// ManagedRuntime initialized once at startup
export const runtime = ManagedRuntime.make(AppLayer)

// Export the runtime type for use in router signatures
export type AppRuntime = typeof runtime
