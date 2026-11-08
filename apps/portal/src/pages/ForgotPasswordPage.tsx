import { requestPasswordReset } from "@/api/auth";
import { RequestLinkForm } from "./AuthForm";

/** `/forgot-password`: email a password reset link. */
export function ForgotPasswordPage() {
  return (
    <RequestLinkForm
      title="Reset your password"
      intro="Enter your account's email address and we'll send you a link to choose a new password."
      submitLabel="Email me a reset link"
      lifetime="1 hour"
      send={(email) => requestPasswordReset(email)}
    />
  );
}
