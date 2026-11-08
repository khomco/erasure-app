import { requestMagicLink } from "@/api/auth";
import { RequestLinkForm } from "./AuthForm";

/** `/sign-in-link`: email a one-time sign-in (magic) link. */
export function SignInLinkPage() {
  return (
    <RequestLinkForm
      title="Email me a sign-in link"
      intro="Enter your account's email address and we'll send you a link that signs you in without a password."
      submitLabel="Email me a link"
      lifetime="15 minutes"
      send={(email) => requestMagicLink(email)}
    />
  );
}
