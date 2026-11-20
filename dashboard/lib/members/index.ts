import "server-only";
import { env } from "../env";
import { keycloakMembers } from "./keycloak";

export type Member = {
  id: string;
  username: string;
  email?: string;
  name: string;
  admin: boolean;
};

export type Org = { id: string; alias: string };

/**
 * Org membership management, implemented per identity provider. The dashboard only shows the
 * Members page when one is configured (MEMBERS_PROVIDER). To support another provider, implement
 * this interface (usually against its admin/management API) and register it below.
 *
 * Callers have already checked that the signed-in user is an admin of `org`; implementations
 * only talk to the provider.
 */
export interface MembersAdapter {
  list(org: Org): Promise<Member[]>;
  /** Add an existing account, found by email. */
  addByEmail(org: Org, email: string): Promise<"added" | "not-found" | "already">;
  remove(org: Org, userId: string): Promise<void>;
  setAdmin(org: Org, userId: string, admin: boolean): Promise<void>;
}

/** The configured adapter, or null when members are managed in the provider's own console. */
export function members(): MembersAdapter | null {
  switch (env.membersProvider()) {
    case "keycloak":
      return keycloakMembers;
    case "none":
      return null;
  }
}
