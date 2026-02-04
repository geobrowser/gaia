/**
 * Proposals router.
 *
 * Provides REST endpoints for querying proposal status.
 */

import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import { Data, Effect, Either } from "effect";
import { Hono } from "hono";
import { describeRoute } from "hono-openapi";

import type { AppRuntime } from "../services/runtime";
import { isValidUuid } from "../utils/uuid";
import { getProposalWithVotes, QueryError } from "./queries";
import { computeProposalStatus, getCurrentTimeSeconds } from "./status";
import type { ProposalStatusResponse } from "./types";

type Database = NodePgDatabase<Record<string, unknown>>;

type AppEnv = {
  Variables: {
    requestId: string;
  };
};

class ValidationError extends Data.TaggedError("ValidationError")<{
  message: string;
}> {}

class NotFoundError extends Data.TaggedError("NotFoundError")<{
  message: string;
}> {}

type ProposalStatusError = ValidationError | NotFoundError | QueryError;

/**
 * Create the proposals router.
 *
 * @param db - Drizzle database instance
 * @param runtime - Effect runtime with telemetry and other services
 * @returns Configured Hono router
 */
export function createProposalsRouter(db: Database, runtime: AppRuntime) {
  const router = new Hono<AppEnv>();

  /**
   * GET /proposals/:id/status
   *
   * Get the computed status of a proposal.
   */
  router.get(
    "/:id/status",
    describeRoute({
      tags: ["Proposals"],
      summary: "Get proposal status",
      description: `
Computes the current status of a proposal based on votes and time remaining.
Status computation matches the smart contract's \`isSupportThresholdReached()\` logic.

**Status Values:**
- \`PROPOSED\`: Voting is active, threshold not yet reached
- \`EXECUTABLE\`: Threshold reached, can be executed on-chain
- \`ACCEPTED\`: Proposal has been executed
- \`REJECTED\`: Voting ended without reaching threshold/quorum
			`.trim(),
      parameters: [
        {
          name: "id",
          in: "path",
          description: "Proposal UUID",
          required: true,
          schema: { type: "string", format: "uuid" },
        },
      ],
      responses: {
        200: {
          description: "Proposal status",
          content: {
            "application/json": {
              schema: { $ref: "#/components/schemas/ProposalStatusResponse" },
            },
          },
        },
        400: {
          description: "Invalid parameter",
          content: {
            "application/json": {
              schema: {
                type: "object",
                properties: {
                  error: { type: "string" },
                  message: { type: "string" },
                },
              },
            },
          },
        },
        404: {
          description: "Proposal not found",
          content: {
            "application/json": {
              schema: {
                type: "object",
                properties: {
                  error: { type: "string" },
                  message: { type: "string" },
                },
              },
            },
          },
        },
        500: {
          description: "Internal server error",
          content: {
            "application/json": {
              schema: {
                type: "object",
                properties: {
                  error: { type: "string" },
                  message: { type: "string" },
                },
              },
            },
          },
        },
      },
    }),
    async (c) => {
      const proposalId = c.req.param("id");
      const requestId = c.get("requestId") ?? "unknown";

      const program = Effect.gen(function* () {
        // Validate proposalId
        if (!isValidUuid(proposalId)) {
          return yield* Effect.fail(
            new ValidationError({
              message: "Proposal ID must be a valid UUID",
            }),
          );
        }

        // Fetch proposal with vote counts
        const proposal = yield* getProposalWithVotes(db, proposalId);

        if (!proposal) {
          return yield* Effect.fail(
            new NotFoundError({
              message: `Proposal '${proposalId}' not found`,
            }),
          );
        }

        // Compute status using pure function
        const nowSeconds = getCurrentTimeSeconds();
        const { status, isQuorumReached, isThresholdReached } =
          computeProposalStatus(proposal, nowSeconds);

        // Compute timing info
        const now = Number(nowSeconds);
        const endTime = Number(proposal.endTime);
        const isVotingEnded = now > endTime;
        const timeRemaining = isVotingEnded ? null : endTime - now;

        // Build response with proper serialization
        const response: ProposalStatusResponse = {
          proposalId: proposal.id,
          status,
          votingMode: proposal.votingMode,
          votes: {
            yes: Number(proposal.yesCount),
            no: Number(proposal.noCount),
            abstain: Number(proposal.abstainCount),
            total: Number(
              proposal.yesCount + proposal.noCount + proposal.abstainCount,
            ),
          },
          thresholds: {
            quorum: proposal.quorum.toString(),
            threshold: proposal.threshold.toString(),
          },
          timing: {
            startTime: Number(proposal.startTime),
            endTime,
            timeRemaining,
            isVotingEnded,
          },
          isQuorumReached,
          isThresholdReached,
          canExecute: status === "EXECUTABLE",
        };

        return response;
      }).pipe(
        Effect.tapError((error) => {
          if (error._tag === "QueryError") {
            return Effect.logError(
              `Database error: operation=${error.operation}, cause=${String(error.cause)}`,
            );
          }
          return Effect.void;
        }),
        Effect.withSpan("GET /proposals/:id/status"),
        Effect.annotateSpans({ requestId, proposalId }),
      );

      const result = await runtime.runPromise(Effect.either(program));

      return Either.match(result, {
        onLeft: (error: ProposalStatusError) => {
          switch (error._tag) {
            case "ValidationError":
              return c.json(
                { error: "Invalid parameter", message: error.message },
                400,
              );
            case "NotFoundError":
              return c.json(
                { error: "Not found", message: error.message },
                404,
              );
            case "QueryError":
              return c.json(
                {
                  error: "Internal server error",
                  message: "An unexpected error occurred",
                },
                500,
              );
          }
        },
        onRight: (response) => c.json(response),
      });
    },
  );

  return router;
}
