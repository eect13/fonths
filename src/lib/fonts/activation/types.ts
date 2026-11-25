/** Progress owner — download vs on-disk register vs remove must not share one sticky bar. */
export type ProgressOwner = "idle" | "download" | "register" | "remove";

export type DownloadJobState = {
  running: boolean;
  paused: boolean;
  mode: ProgressOwner;
  /** Who owns done/total/current — cancel/finish of another owner must not leave Registering… */
  owner: ProgressOwner;
  done: number;
  total: number;
  failed: number;
  skipped: number;
  current: string;
  failedNames: string[];
  failedDetails: string[];
  settledNames?: string[];
};

export type FontsourceOfferResult = {
  family: string;
  added: number;
  settled: boolean;
  message: string;
};

export type GdiSessionState = {
  liveFamilies: string[];
  liveFamilyCount: number;
  liveFaceCount: number;
  stagedMapCount: number;
  gdiObjects: number;
  gdiObjectsPeak: number;
  gdiQuota: number;
  fastResume: boolean;
  bootRunning: boolean;
  bootDone: boolean;
};

export type DiskFamilyInfo = {
  name: string;
  bytes: number;
  files: number;
  corrupt?: number;
  incomplete?: boolean;
  /** Known GDI-incapable + intact — calm Settled (not Incomplete, not Activated). */
  settled?: boolean;
  has_complete?: boolean;
  has_variable?: boolean;
  missing_variable?: boolean;
  undersized?: boolean;
};

export type SyncDocsResult = {
  familiesSeen: number;
  familiesPurged: number;
  staticsDeleted: number;
  locked: number;
  /** Only true when Rust reports cancelled — never OR'd with sticky pending (206aa amend). */
  cancelled: boolean;
  /**
   * Cancel was clicked but Rust already finished (`cancelled: false`).
   * Callers must toast honest late-cancel copy — never "Documents refresh cancelled".
   */
  cancelArrivedLate: boolean;
};

export type RepairResult = {
  queued: number;
  healed: number;
  locked: number;
  write_failed: number;
  var_ensured: number;
};

export type CancelDownloadOpts = {
  /** True when bar docs Cancel chrome (button, in-window Escape, or tray) — always docs path. */
  fromDocsCancelChrome?: boolean;
};
