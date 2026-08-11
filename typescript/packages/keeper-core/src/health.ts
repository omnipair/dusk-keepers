export type HealthStatus = "healthy" | "degraded" | "unhealthy";
export type KeeperMode = "shadow" | "live";
export type RuntimeLanguage = "rust" | "typescript";
export type ProtocolLockStatus = "draft" | "captured" | "frozen";
export type DependencyState = "ok" | "degraded" | "down" | "disabled" | "unknown";
export type CircuitBreakerState = "closed" | "open" | "half_open";
export type ParityStatus = "matching" | "mismatched" | "unknown";

export interface HealthProvenance {
  readonly schemaVersion: 1;
  readonly status: HealthStatus;
  readonly mode: KeeperMode;
  readonly service: string;
  readonly profile: string;
  readonly runtime: {
    readonly language: RuntimeLanguage;
    readonly version: string;
    readonly buildSha: string | null;
  };
  readonly protocol: {
    readonly revision: string;
    readonly lockStatus: ProtocolLockStatus;
    readonly lockSha256: string;
    readonly sourceWorktreeFingerprintSha256: string;
  };
  readonly dependencies: {
    readonly rpc: DependencyState;
    readonly websocket: DependencyState;
    readonly indexer: DependencyState;
    readonly database: DependencyState;
    readonly leaseStore: DependencyState;
    readonly signer: DependencyState;
  };
  readonly slots: {
    readonly rpc: number | null;
    readonly indexer: number | null;
    readonly lastReconciled: number | null;
    readonly lag: number | null;
  };
  readonly work: {
    readonly pending: number;
    readonly inFlight: number;
    readonly awaitingReconciliation: number;
    readonly oldestAttemptAgeSeconds: number | null;
  };
  readonly circuitBreaker: {
    readonly state: CircuitBreakerState;
    readonly reason: string | null;
  };
  readonly parity: {
    readonly status: ParityStatus;
    readonly lastComparedAt: string | null;
    readonly mismatchCount: number;
  };
  readonly timestamps: {
    readonly startedAt: string;
    readonly lastScanAt: string | null;
    readonly lastSuccessAt: string | null;
    readonly generatedAt: string;
  };
}

