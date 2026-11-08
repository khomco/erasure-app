import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { fetchMe, type Account } from "@/api/auth";
import { fetchSeats, type Entitlement } from "@/api/orders";
import {
  fetchStations,
  fetchVendorRoot,
  isInstanceKeyId,
  licenceDownloadPath,
  registerStation,
  reissueStation,
  releaseStation,
  type Licence,
  type Station,
  type VendorRootKey,
} from "@/api/stations";
import { SignInPrompt } from "./AccountPage";
import { Field, FormError } from "./AuthForm";
import { SCOPE_LABELS } from "./OfferingForm";

/** The date part of an RFC 3339 UTC timestamp: `2027-11-08`. */
function day(timestamp: string): string {
  return timestamp.slice(0, 10);
}

/** Seats of `entitlement` held by stations that are not released. */
export function seatsUsed(entitlement: Entitlement, stations: Station[]): number {
  return stations.filter((s) => s.entitlement_id === entitlement.id && s.released_at === null)
    .length;
}

/** A current entitlement and how many of its seats are free. */
export interface SeatChoice {
  entitlement: Entitlement;
  free: number;
}

/** The current entitlements a station could be registered against, newest first. */
export function seatChoices(entitlements: Entitlement[], stations: Station[]): SeatChoice[] {
  return entitlements
    .filter((e) => e.current)
    .map((e) => ({ entitlement: e, free: Math.max(0, e.grants.seats - seatsUsed(e, stations)) }));
}

/** The entitlement to preselect: the only one with a free seat, else none. */
export function preselected(choices: SeatChoice[]): string {
  const open = choices.filter((c) => c.free > 0);
  return open.length === 1 ? open[0].entitlement.id : "";
}

/** "Bench 1 (bench-1)", "bench-1", "Bench 1" or the key id. */
export function stationName(s: Station): string {
  if (s.label && s.station_id) return `${s.label} (${s.station_id})`;
  return s.label ?? s.station_id ?? s.instance_key_id;
}

/**
 * "Released on …", "Revoked on …", "Renewal available …", "Expired on …",
 * "Licensed to …" or "No licence", as of `now`. A revocation's reason is the
 * vendor's and never reaches this page.
 */
export function stationState(s: Station, now: Date = new Date()): string {
  if (s.released_at) return `Released on ${day(s.released_at)}`;
  if (s.licence?.status === "revoked" && s.licence.revoked_at) {
    return `Revoked on ${day(s.licence.revoked_at)}`;
  }
  if (s.licence?.status === "active") {
    const expired = Date.parse(s.licence.not_after) <= now.getTime();
    if (s.renewal_available) {
      return `Renewal available (current licence ${expired ? "expired on" : "to"} ${day(
        s.licence.not_after,
      )})`;
    }
    return expired
      ? `Expired on ${day(s.licence.not_after)}`
      : `Licensed to ${day(s.licence.not_after)}`;
  }
  return "No licence";
}

/** The `wipestation serve` flags that install a downloaded licence. */
export function installCommand(root: VendorRootKey | null): string {
  return `wipestation serve --station-id <station id> --license <licence file> --vendor-root-b64 ${
    root?.public_key_b64 ?? "<vendor root public key>"
  }`;
}

/**
 * `/stations`: the organization's registered stations and their licences.
 * Every member sees the list, downloads licence files and reads how to
 * install one; owners also register stations against a free seat, issue
 * renewed licences after an entitlement renews, and release stations.
 */
export function StationsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => fetchMe(), retry: false });
  return (
    <section className="space-y-6">
      <p className="text-sm">
        <Link to="/" className="text-indigo-400 hover:underline">
          Back to the portal
        </Link>
      </p>
      <h1 className="text-xl font-semibold">Stations</h1>
      {me.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={me.error} />
      {me.data === null && <SignInPrompt />}
      {me.data && <StationsSection account={me.data} />}
    </section>
  );
}

function StationsSection({ account }: { account: Account }) {
  const owner = account.user.role === "owner";
  const stations = useQuery({ queryKey: ["stations"], queryFn: () => fetchStations() });
  const seats = useQuery({ queryKey: ["seats"], queryFn: () => fetchSeats() });
  const root = useQuery({ queryKey: ["vendor-root"], queryFn: () => fetchVendorRoot() });
  return (
    <>
      <p className="flex gap-4 text-sm">
        <Link to="/seats" className="text-indigo-400 hover:underline">
          Licence seats
        </Link>
        <Link to="/buy" className="text-indigo-400 hover:underline">
          Buy
        </Link>
      </p>
      <SupersessionNote />
      {stations.isPending && <p className="text-sm text-slate-400">Loading…</p>}
      <FormError error={stations.error} />
      {stations.data && <StationTable stations={stations.data} canRelease={owner} />}
      {!owner && (
        <p className="text-sm text-slate-400">
          Only an organization owner can register, renew or release stations.
        </p>
      )}
      <FormError error={seats.error} />
      {owner && stations.data && seats.data && (
        <RegisterForm
          choices={seatChoices(seats.data.entitlements, stations.data)}
          issuance={root.isSuccess && root.data !== null}
        />
      )}
      <KeyIdHowTo />
      <FormError error={root.error} />
      {root.isSuccess && <VendorRootSection root={root.data} />}
    </>
  );
}

/** What releasing and renewing a station do and do not do, stated plainly. */
export function SupersessionNote() {
  return (
    <div className="space-y-2 rounded-md border border-slate-800 bg-slate-900 p-3 text-sm text-slate-300">
      <p>
        Releasing a station frees its seat and marks its licence superseded here. The licence file
        already on that station keeps working until its own expiry date: the hub does not send
        revocation or supersession to stations.
      </p>
      <p>
        When an entitlement renews, its stations show Renewal available and an owner issues each
        one a renewed licence here. The station uses the renewed licence only after the renewed
        file is installed on it. The old file keeps working until its own expiry date.
      </p>
      <p>
        A station whose licence the vendor revoked shows Revoked: it cannot be renewed or its
        licence downloaded here, and it keeps its seat until an owner releases it. Contact the
        vendor about a revoked licence.
      </p>
    </div>
  );
}

/** What a renewed licence needs next, with its download link. */
export function RenewedNotice({ licence }: { licence: Licence }) {
  return (
    <p className="text-sm whitespace-normal text-emerald-400">
      Renewed licence issued, to {day(licence.not_after)}.{" "}
      <a href={licenceDownloadPath(licence.id)} className="text-indigo-400 hover:underline">
        Download the renewed licence
      </a>{" "}
      and install it on the station: until then it keeps using the old file, which works until
      its own expiry date.
    </p>
  );
}

/**
 * A station's renewal, when one is available: owners issue the renewed
 * licence and then download it; members are told an owner can issue it.
 */
export function RenewalAction({ station: s, canRenew }: { station: Station; canRenew: boolean }) {
  const queryClient = useQueryClient();
  const reissue = useMutation({
    mutationFn: () => reissueStation(s.id),
    // Also after a refusal: another owner or tab may have reissued or released it.
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["stations"] }),
  });
  if (s.released_at !== null) return null;
  if (reissue.isSuccess) return <RenewedNotice licence={reissue.data.licence} />;
  if (!s.renewal_available) return null;
  if (!canRenew) {
    return <span className="text-slate-400">An owner can issue the renewed licence.</span>;
  }
  return (
    <>
      <button
        type="button"
        disabled={reissue.isPending}
        onClick={() => reissue.mutate()}
        className="text-emerald-400 hover:underline disabled:opacity-50"
      >
        Issue renewed licence
      </button>
      <FormError error={reissue.error} />
    </>
  );
}

function StationTable({ stations, canRelease }: { stations: Station[]; canRelease: boolean }) {
  if (stations.length === 0) {
    return <p className="text-sm text-slate-400">No stations registered yet.</p>;
  }
  return (
    <table className="w-full text-left text-sm">
      <thead className="text-slate-400">
        <tr>
          <th className="py-1 pr-3 font-medium">Station</th>
          <th className="py-1 pr-3 font-medium">Instance key id</th>
          <th className="py-1 pr-3 font-medium">Seat from</th>
          <th className="py-1 pr-3 font-medium">State</th>
          <th className="py-1 font-medium" />
        </tr>
      </thead>
      <tbody className="divide-y divide-slate-800">
        {stations.map((s) => (
          <StationRow key={s.id} station={s} canRelease={canRelease} />
        ))}
      </tbody>
    </table>
  );
}

/** One station's row: its state, download, renewal and (owners) release. */
export function StationRow({ station: s, canRelease }: { station: Station; canRelease: boolean }) {
  const queryClient = useQueryClient();
  const release = useMutation({
    mutationFn: () => releaseStation(s.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["stations"] }),
  });
  const released = s.released_at !== null;
  return (
    <tr className={released ? "text-slate-500" : ""}>
      <td className="py-1 pr-3">
        {stationName(s)}
        {s.site && <span className="text-slate-400"> · site {s.site}</span>}
      </td>
      <td className="py-1 pr-3 font-mono text-xs">{s.instance_key_id}</td>
      <td className="py-1 pr-3">
        {s.offering_name}{" "}
        <span className="text-slate-400">({SCOPE_LABELS[s.scope].toLowerCase()})</span>
      </td>
      <td className="py-1 pr-3">{stationState(s)}</td>
      <td className="space-x-3 py-1 whitespace-nowrap">
        {s.licence && !released && s.licence.status !== "revoked" && (
          <a href={licenceDownloadPath(s.licence.id)} className="text-indigo-400 hover:underline">
            Download licence
          </a>
        )}
        <RenewalAction station={s} canRenew={canRelease} />
        {canRelease && !released && (
          <button
            type="button"
            disabled={release.isPending}
            onClick={() => {
              if (
                window.confirm(
                  `Release ${stationName(s)}? Its seat becomes free. The licence file on the ` +
                    "station keeps working until it expires.",
                )
              ) {
                release.mutate();
              }
            }}
            className="text-rose-400 hover:underline disabled:opacity-50"
          >
            Release
          </button>
        )}
        <FormError error={release.error} />
      </td>
    </tr>
  );
}

function RegisterForm({ choices, issuance }: { choices: SeatChoice[]; issuance: boolean }) {
  const queryClient = useQueryClient();
  const [entitlementId, setEntitlementId] = useState(() => preselected(choices));
  const [keyId, setKeyId] = useState("");
  const [stationId, setStationId] = useState("");
  const [label, setLabel] = useState("");
  const [site, setSite] = useState("");
  const register = useMutation({
    mutationFn: () =>
      registerStation({
        entitlement_id: entitlementId,
        instance_key_id: keyId,
        station_id: stationId,
        label,
        // The site field shows, and counts, only for a site entitlement.
        site: chosen?.entitlement.grants.scope === "site" ? site : undefined,
      }),
    onSuccess: () => {
      setKeyId("");
      setStationId("");
      setLabel("");
      setSite("");
      return queryClient.invalidateQueries({ queryKey: ["stations"] });
    },
  });
  const chosen = choices.find((c) => c.entitlement.id === entitlementId);
  const machine = chosen?.entitlement.grants.scope === "machine";
  const keyLooksWrong = keyId.trim() !== "" && !isInstanceKeyId(keyId);
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    register.mutate();
  };

  if (choices.length === 0) {
    return (
      <p className="text-sm text-slate-400">
        To register a station you need a current licence seat.{" "}
        <Link to="/buy" className="text-indigo-400 hover:underline">
          Buy one
        </Link>
        .
      </p>
    );
  }
  return (
    <form onSubmit={onSubmit} className="space-y-3 rounded-md border border-slate-800 p-4">
      <h2 className="font-medium">Register a station</h2>
      {!issuance && (
        <p role="alert" className="text-sm text-amber-300">
          Licence issuance is unavailable: this hub has no vendor root configured.
        </p>
      )}
      <label className="block space-y-1 text-sm">
        <span className="text-slate-300">Seat from</span>
        <select
          name="entitlement_id"
          required
          value={entitlementId}
          onChange={(e) => setEntitlementId(e.target.value)}
          className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-slate-100"
        >
          <option value="">Choose an entitlement…</option>
          {choices.map((c) => (
            <option key={c.entitlement.id} value={c.entitlement.id} disabled={c.free === 0}>
              {c.entitlement.offering_name} ({SCOPE_LABELS[c.entitlement.grants.scope].toLowerCase()}
              , {c.free} of {c.entitlement.grants.seats} free, to {day(c.entitlement.term_end)})
            </option>
          ))}
        </select>
      </label>
      <Field
        label="Instance key id (ed25519:…)"
        name="instance_key_id"
        value={keyId}
        onChange={setKeyId}
      />
      {keyLooksWrong && (
        <p className="text-sm text-amber-300">
          That does not look like an instance key id: it is ed25519: followed by 22 characters.
        </p>
      )}
      <Field
        label={
          machine
            ? "Station id (its --station-id, or standalone if it was started without one)"
            : "Station id (optional)"
        }
        name="station_id"
        required={machine}
        value={stationId}
        onChange={setStationId}
      />
      {machine && (
        <p className="text-xs text-slate-400">
          A machine licence is bound to this id. Started under any other id, the station still
          erases but marks its certificates evaluation.
        </p>
      )}
      <Field label="Label (optional)" name="label" required={false} value={label} onChange={setLabel} />
      {chosen?.entitlement.grants.scope === "site" && (
        <Field
          label="Site (optional; defaults to your organization)"
          name="site"
          required={false}
          value={site}
          onChange={setSite}
        />
      )}
      <button
        type="submit"
        disabled={register.isPending || !issuance || !chosen || chosen.free === 0}
        className="rounded-md bg-indigo-600 px-3 py-1 text-sm font-medium hover:bg-indigo-500 disabled:opacity-50"
      >
        Register and issue licence
      </button>
      <FormError error={register.error} />
      {register.isSuccess && (
        <p className="text-sm text-emerald-400">
          Registered.{" "}
          <a
            href={licenceDownloadPath(register.data.licence.id)}
            className="text-indigo-400 hover:underline"
          >
            Download its licence
          </a>{" "}
          and install it on the station.
        </p>
      )}
    </form>
  );
}

/** How to read a station's instance key id. */
export function KeyIdHowTo() {
  return (
    <div className="space-y-2 text-sm">
      <h2 className="font-medium">Finding a station's instance key id</h2>
      <p className="text-slate-300">Each station has its own signing key. Read its id on the station:</p>
      <ul className="list-disc space-y-1 pl-5 text-slate-300">
        <li>
          from its API: <code className="font-mono text-xs">GET /api/public_key</code> answers it as{" "}
          <code className="font-mono text-xs">public_key_id</code>;
        </li>
        <li>
          or from its startup log, on the line{" "}
          <code className="font-mono text-xs">signing key public id: ed25519:…</code>.
        </li>
      </ul>
    </div>
  );
}

/** The vendor root public key, and how a station installs a licence with it. */
export function VendorRootSection({ root }: { root: VendorRootKey | null }) {
  return (
    <div className="space-y-2 text-sm">
      <h2 className="font-medium">Vendor root public key</h2>
      {root === null ? (
        <p className="text-slate-400">
          This hub has no vendor root configured, so it cannot issue licences.
        </p>
      ) : (
        <>
          <p className="text-slate-300">
            Licences are signed by this key. Stations check a licence against it, and auditors
            verify certificates with it.
          </p>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            <dt className="text-slate-400">Key id</dt>
            <dd className="font-mono text-xs break-all">{root.public_key_id}</dd>
            <dt className="text-slate-400">Public key</dt>
            <dd className="font-mono text-xs break-all">{root.public_key_b64}</dd>
          </dl>
        </>
      )}
      <p className="text-slate-300">
        Install a downloaded licence by starting the station with this line. For a machine
        licence, use the station id it was registered with; a site licence works under any
        station id.
      </p>
      <pre className="overflow-x-auto rounded-md bg-slate-900 p-2 font-mono text-xs">
        {installCommand(root)}
      </pre>
    </div>
  );
}
