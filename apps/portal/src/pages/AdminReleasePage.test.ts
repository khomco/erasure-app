import { describe, expect, it } from "vitest";
import type { Release, ReleaseArtifact } from "@/api/adminReleases";
import { releaseActions, statusLine, suggestedFileName, uploadProblem } from "./AdminReleasePage";
import { fileCount, statusDate } from "./AdminReleasesPage";

const artifact: ReleaseArtifact = {
  id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  platform: "linux-x86_64",
  filename: "wipestation",
  size_bytes: 3,
  sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  created_at: "2026-11-08T10:01:00Z",
};

const draft: Release = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  version: "0.2.0",
  notes: "First build.",
  channel: "stable",
  status: "draft",
  created_at: "2026-11-08T10:00:00Z",
  published_at: null,
  withdrawn_at: null,
  artifacts: [],
};
const withFile: Release = { ...draft, artifacts: [artifact] };
const published: Release = { ...withFile, status: "published", published_at: "2026-11-09T10:00:00Z" };
const withdrawn: Release = { ...published, status: "withdrawn", withdrawn_at: "2026-11-10T10:00:00Z" };

describe("release lifecycle on the page", () => {
  it("offers only the hub's allowed steps", () => {
    expect(releaseActions(draft)).toEqual({ editFiles: true, publish: false, withdraw: true });
    expect(releaseActions(withFile)).toEqual({ editFiles: true, publish: true, withdraw: true });
    expect(releaseActions(published)).toEqual({ editFiles: false, publish: false, withdraw: true });
    expect(releaseActions(withdrawn)).toEqual({ editFiles: false, publish: false, withdraw: false });
  });

  it("says where each release stands", () => {
    expect(statusLine(draft)).toMatch(/upload at least one file/);
    expect(statusLine(withFile)).toMatch(/until it is published/);
    expect(statusLine(published)).toMatch(/frozen/);
    expect(statusLine(withdrawn)).toMatch(/stays listed/);
  });

  it("refuses a doomed upload before sending it", () => {
    const file = { size: 10 };
    expect(uploadProblem(draft, "linux-x86_64", file, "wipestation")).toBeNull();
    expect(uploadProblem(draft, "Linux", file, "wipestation")).toMatch(/lowercase/);
    expect(uploadProblem(withFile, "linux-x86_64", file, "w")).toMatch(
      /already has a linux-x86_64 file/,
    );
    expect(uploadProblem(withFile, "macos-aarch64", file, "w")).toBeNull();
    expect(uploadProblem(draft, "linux-x86_64", null, "w")).toBe("Choose a file.");
    expect(uploadProblem(draft, "linux-x86_64", { size: 0 }, "w")).toBe("The file is empty.");
    expect(uploadProblem(draft, "linux-x86_64", { size: 1 }, ".w")).toMatch(/Use a file name/);
    expect(uploadProblem(draft, "linux-x86_64", { size: 1 }, "")).toMatch(/Use a file name/);
  });

  it("suggests a file name the hub accepts from the picked one", () => {
    expect(suggestedFileName("wipestation (1).tar.gz")).toBe("wipestation-1.tar.gz");
    expect(suggestedFileName("wipestation")).toBe("wipestation");
    expect(suggestedFileName(".hidden build")).toBe("hidden-build");
    expect(suggestedFileName("naïve.exe")).toBe("na-ve.exe");
    expect(suggestedFileName("a".repeat(200))).toHaveLength(128);
  });
});

describe("the release list", () => {
  it("counts files and dates each status", () => {
    expect(fileCount(draft)).toBe("no files");
    expect(fileCount(withFile)).toBe("1 file");
    expect(fileCount({ ...withFile, artifacts: [artifact, { ...artifact, id: "b" }] })).toBe(
      "2 files",
    );
    expect(statusDate(draft)).toBe("2026-11-08");
    expect(statusDate(published)).toBe("2026-11-09");
    expect(statusDate(withdrawn)).toBe("2026-11-10");
  });
});
