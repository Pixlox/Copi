import { useEffect, useState, useCallback, useRef, type MouseEvent as ReactMouseEvent, type DragEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Upload,
  Download,
  X,
  Check,
  AlertCircle,
  File,
  FileText,
  FileImage,
  FileVideo,
  FileAudio,
  FileArchive,
  Trash2,
  RefreshCw,
} from "lucide-react";
import { isMacPlatform } from "../utils/platform";

// ════════════════════════════════════════════════════════════════════════════
// Types
// ════════════════════════════════════════════════════════════════════════════

interface WormholeFile {
  id: string;
  file_name: string;
  file_size: number;
  mime_type: string | null;
  status: "pending" | "uploading" | "available" | "downloading" | "completed" | "expired" | "cancelled" | "failed";
  is_local: boolean;
  origin_device_id: string;
  origin_device_name: string | null;
  bytes_transferred: number;
  transfer_started_at: string | null;
  transfer_completed_at: string | null;
  local_path: string | null;
  created_at: string;
  expires_at: string;
}

interface TransferProgress {
  file_id: string;
  file_name: string;
  bytes_transferred: number;
  total_bytes: number;
  percent_complete: number;
  speed_bytes_per_sec: number;
  estimated_seconds_remaining: number;
  is_upload: boolean;
}

// ════════════════════════════════════════════════════════════════════════════
// Helpers
// ════════════════════════════════════════════════════════════════════════════

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`;
}

function formatEta(seconds: number): string {
  if (seconds <= 0) return "";
  if (seconds < 60) return `${Math.ceil(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${Math.ceil(seconds % 60)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

function formatTimeRemaining(expiresAtIso: string): string {
  const expiresAtMs = new Date(expiresAtIso).getTime();
  if (!Number.isFinite(expiresAtMs)) return "";
  const remaining = Math.max(0, Math.floor((expiresAtMs - Date.now()) / 1000));
  if (remaining <= 0) return "Expired";
  if (remaining < 3600) return `${Math.floor(remaining / 60)}m left`;
  if (remaining < 86400) return `${Math.floor(remaining / 3600)}h left`;
  return `${Math.floor(remaining / 86400)}d left`;
}

function getFileIcon(mimeType: string | null, fileName: string) {
  if (mimeType) {
    if (mimeType.startsWith("image/")) return FileImage;
    if (mimeType.startsWith("video/")) return FileVideo;
    if (mimeType.startsWith("audio/")) return FileAudio;
    if (mimeType.startsWith("text/")) return FileText;
    if (mimeType.includes("zip") || mimeType.includes("archive") || mimeType.includes("compressed")) return FileArchive;
  }
  
  const ext = fileName.split(".").pop()?.toLowerCase();
  if (ext) {
    if (["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico"].includes(ext)) return FileImage;
    if (["mp4", "mov", "avi", "mkv", "webm"].includes(ext)) return FileVideo;
    if (["mp3", "wav", "flac", "aac", "ogg", "m4a"].includes(ext)) return FileAudio;
    if (["txt", "md", "json", "xml", "csv", "log"].includes(ext)) return FileText;
    if (["zip", "rar", "7z", "tar", "gz", "bz2"].includes(ext)) return FileArchive;
  }
  
  return File;
}

// ════════════════════════════════════════════════════════════════════════════
// Components
// ════════════════════════════════════════════════════════════════════════════

function DropZone({ onFilesDropped, onSelectFiles, isDragActive, setIsDragActive }: {
  onFilesDropped: (paths: string[]) => void;
  onSelectFiles: () => void;
  isDragActive: boolean;
  setIsDragActive: (active: boolean) => void;
}) {
  const dropRef = useRef<HTMLDivElement>(null);
  const dragCounter = useRef(0);

  const handleDragEnter = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounter.current++;
    if (e.dataTransfer?.types.includes("Files")) {
      setIsDragActive(true);
    }
  }, [setIsDragActive]);

  const handleDragLeave = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounter.current--;
    if (dragCounter.current === 0) {
      setIsDragActive(false);
    }
  }, [setIsDragActive]);

  const handleDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDrop = useCallback(async (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounter.current = 0;
    setIsDragActive(false);

    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;

    // Get file paths from the dropped files
    // In Tauri, we need to use the webkitRelativePath or handle via backend
    const paths: string[] = [];
    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      // @ts-expect-error - path is available in Tauri
      if (file.path) {
        // @ts-expect-error - path is available in Tauri
        paths.push(file.path);
      }
    }
    
    if (paths.length > 0) {
      onFilesDropped(paths);
    }
  }, [onFilesDropped, setIsDragActive]);

  return (
    <div
      ref={dropRef}
      className={`wormhole-dropzone ${isDragActive ? "active" : ""}`}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <div className="wormhole-dropzone-content">
        <div className={`wormhole-dropzone-icon ${isDragActive ? "pulse" : ""}`}>
          <Upload size={32} strokeWidth={1.5} />
        </div>
        <span className="wormhole-dropzone-text">
          {isDragActive ? "Drop files to send" : "Drag files here to send"}
        </span>
        <span className="wormhole-dropzone-hint">
          Drag files in, or choose them manually
        </span>
        <button
          type="button"
          className="wormhole-dropzone-btn"
          onClick={onSelectFiles}
          data-no-drag
        >
          Select Files
        </button>
      </div>
    </div>
  );
}

function normalizeWormholeFile(file: WormholeFile): WormholeFile {
  const status = file.status;
  const normalizedStatus: WormholeFile["status"] = status === "pending" && file.is_local
    ? "available"
    : status;

  return {
    ...file,
    status: normalizedStatus,
  };
}

function FileCard({ file, progress, onDownload, onRetract, onCancel }: {
  file: WormholeFile;
  progress?: TransferProgress;
  onDownload: () => void;
  onRetract: () => void;
  onCancel: () => void;
}) {
  const Icon = getFileIcon(file.mime_type, file.file_name);
  const isUploading = file.status === "uploading";
  const isDownloading = file.status === "downloading";
  const isTransferring = isUploading || isDownloading;
  const isAvailable = file.status === "available";
  const isPending = file.status === "pending";
  const isCompleted = file.status === "completed";
  const isFailed = file.status === "failed";
  const isExpired = file.status === "expired";
  const isCancelled = file.status === "cancelled";

  const progressPercent = progress 
    ? Math.min(100, progress.percent_complete)
    : 0;

  const canRetract = file.is_local && (isPending || isAvailable || isUploading);
  const canDownload = !file.is_local && (isPending || isAvailable);

  return (
    <div className={`wormhole-file-card ${file.status}`}>
      {/* Progress bar background for transfers */}
      {isTransferring && (
        <div 
          className="wormhole-file-progress-bar" 
          style={{ width: `${progressPercent}%` }}
        />
      )}
      
      <div className="wormhole-file-icon">
        <Icon size={24} strokeWidth={1.5} />
      </div>
      
      <div className="wormhole-file-info">
        <span className="wormhole-file-name" title={file.file_name}>
          {file.file_name}
        </span>
        <div className="wormhole-file-meta">
          <span className="wormhole-file-size">{formatBytes(file.file_size)}</span>
          {!file.is_local && file.origin_device_name && (
            <span className="wormhole-file-source">from {file.origin_device_name}</span>
          )}
          {isTransferring && progress && (
            <>
              <span className="wormhole-file-speed">{formatSpeed(progress.speed_bytes_per_sec)}</span>
              {progress.estimated_seconds_remaining > 0 && (
                <span className="wormhole-file-eta">{formatEta(progress.estimated_seconds_remaining)}</span>
              )}
            </>
          )}
          {(isPending || isAvailable) && !file.is_local && (
            <span className="wormhole-file-expiry">{formatTimeRemaining(file.expires_at)}</span>
          )}
          {isCompleted && (
            <span className="wormhole-file-completed">Completed</span>
          )}
          {isFailed && (
            <span className="wormhole-file-failed">Transfer failed</span>
          )}
          {isExpired && (
            <span className="wormhole-file-expired">Expired</span>
          )}
          {isCancelled && (
            <span className="wormhole-file-expired">Cancelled</span>
          )}
        </div>
      </div>
      
      <div className="wormhole-file-actions">
        {/* Local pending file: can retract */}
        {canRetract && (
          <button 
            className="wormhole-file-action danger" 
            onClick={onRetract}
            title="Retract offer"
          >
            <X size={16} />
          </button>
        )}
        
        {/* Remote pending file: can download */}
        {canDownload && (
          <button 
            className="wormhole-file-action primary" 
            onClick={onDownload}
            title="Download"
          >
            <Download size={16} />
          </button>
        )}
        
        {/* Transferring: can cancel */}
        {isDownloading && (
          <button 
            className="wormhole-file-action danger" 
            onClick={onCancel}
            title="Cancel transfer"
          >
            <X size={16} />
          </button>
        )}
        
        {/* Completed indicator */}
        {isCompleted && (
          <div className="wormhole-file-status-icon success">
            <Check size={16} />
          </div>
        )}
        
        {/* Failed indicator */}
        {isFailed && (
          <div className="wormhole-file-status-icon error">
            <AlertCircle size={16} />
          </div>
        )}
      </div>
    </div>
  );
}

// ════════════════════════════════════════════════════════════════════════════
// Main Component
// ════════════════════════════════════════════════════════════════════════════

export default function Wormhole() {
  const [files, setFiles] = useState<WormholeFile[]>([]);
  const [progress, setProgress] = useState<Map<string, TransferProgress>>(new Map());
  const [isDragActive, setIsDragActive] = useState(false);
  const [pendingCount, setPendingCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const lastOfferFingerprintRef = useRef<{ key: string; at: number } | null>(null);

  // Fetch files
  const refreshFiles = useCallback(async () => {
    try {
      const result = await invoke<WormholeFile[]>("wormhole_list_files");
      setFiles(result.map(normalizeWormholeFile));
      setError(null);
    } catch (e) {
      console.error("[Wormhole] Failed to fetch files:", e);
      setError(typeof e === "string" ? e : "Failed to load files");
    }
  }, []);

  // Fetch pending count for badge
  const refreshPendingCount = useCallback(async () => {
    try {
      const count = await invoke<number>("wormhole_get_pending_count");
      setPendingCount(count);
    } catch (e) {
      console.error("[Wormhole] Failed to get pending count:", e);
    }
  }, []);

  // Initial load
  useEffect(() => {
    refreshFiles();
    refreshPendingCount();
  }, [refreshFiles, refreshPendingCount]);

  // Listen for events
  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    const refreshAll = () => {
      refreshFiles();
      refreshPendingCount();
    };

    // New file offered (from us)
    unlisteners.push(listen("wormhole://file-offered", () => {
      refreshAll();
    }));

    // New offer received from a peer
    unlisteners.push(listen("wormhole://offer-received", () => {
      refreshAll();
    }));

    // File retracted
    unlisteners.push(listen("wormhole://file-retracted", () => {
      refreshAll();
    }));

    // Remote retraction / expiry
    unlisteners.push(listen("wormhole://offer-retracted", () => {
      refreshAll();
    }));
    unlisteners.push(listen("wormhole://file-expired", () => {
      refreshAll();
    }));

    // Transfer progress
    unlisteners.push(listen<TransferProgress>("wormhole://transfer-progress", (event) => {
      setProgress(prev => {
        const next = new Map(prev);
        next.set(event.payload.file_id, event.payload);
        return next;
      });
    }));

    // Transfer completed
    unlisteners.push(listen<unknown>("wormhole://transfer-complete", (event) => {
      let fileId: string | null = null;
      if (typeof event.payload === "string") {
        fileId = event.payload;
      } else if (
        event.payload &&
        typeof event.payload === "object" &&
        "file_id" in event.payload &&
        typeof (event.payload as { file_id?: unknown }).file_id === "string"
      ) {
        fileId = (event.payload as { file_id: string }).file_id;
      }

      if (!fileId) {
        refreshAll();
        return;
      }

      setProgress(prev => {
        const next = new Map(prev);
        next.delete(fileId);
        return next;
      });
      refreshAll();
    }));

    // Backward-compatible completed event name
    unlisteners.push(listen<{ file_id: string }>("wormhole://transfer-completed", (event) => {
      setProgress(prev => {
        const next = new Map(prev);
        next.delete(event.payload.file_id);
        return next;
      });
      refreshAll();
    }));

    // Transfer failed
    unlisteners.push(listen<{ file_id: string; reason: string }>("wormhole://transfer-failed", (event) => {
      setProgress(prev => {
        const next = new Map(prev);
        next.delete(event.payload.file_id);
        return next;
      });
      refreshAll();
    }));

    return () => {
      unlisteners.forEach(p => p.then(fn => fn()));
    };
  }, [refreshFiles, refreshPendingCount]);

  // Handle file drop
  const handleFilesDropped = useCallback(async (paths: string[]) => {
    const uniquePaths = Array.from(
      new Set(paths.map((p) => p.trim()).filter((p) => p.length > 0))
    );

    if (uniquePaths.length === 0) return;

    const fingerprint = uniquePaths.slice().sort().join("|");
    const now = Date.now();
    const previous = lastOfferFingerprintRef.current;
    if (previous && previous.key === fingerprint && now - previous.at < 1200) {
      return;
    }
    lastOfferFingerprintRef.current = { key: fingerprint, at: now };

    setError(null);

    for (const path of uniquePaths) {
      try {
        await invoke("wormhole_offer_file", { path });
      } catch (e) {
        console.error("[Wormhole] Failed to offer file:", e);
        setError(typeof e === "string" ? e : "Failed to offer file");
      }
    }
    refreshFiles();
    refreshPendingCount();
  }, [refreshFiles, refreshPendingCount]);

  // Native picker button
  const handleSelectFiles = useCallback(async () => {
    try {
      const selected = await open({
        title: "Select files for Wormhole",
        multiple: true,
        directory: false,
      });

      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      await handleFilesDropped(paths);
    } catch (e) {
      console.error("[Wormhole] Failed to open file picker:", e);
      setError(typeof e === "string" ? e : "Failed to open file picker");
    }
  }, [handleFilesDropped]);

  // Native drag-and-drop listener (reliable on Tauri desktop)
  useEffect(() => {
    let dragDepth = 0;
    const unlistenPromise = getCurrentWindow().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter") {
        dragDepth += 1;
        setIsDragActive(true);
        return;
      }

      if (payload.type === "over") {
        setIsDragActive(true);
        return;
      }

      if (payload.type === "leave") {
        dragDepth = Math.max(0, dragDepth - 1);
        if (dragDepth === 0) {
          setIsDragActive(false);
        }
        return;
      }

      if (payload.type === "drop") {
        dragDepth = 0;
        setIsDragActive(false);
        void handleFilesDropped(payload.paths);
      }
    });

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [handleFilesDropped]);

  // Handle download request
  const handleDownload = useCallback(async (fileId: string) => {
    try {
      await invoke("wormhole_request_download", { fileId });
    } catch (e) {
      console.error("[Wormhole] Failed to request download:", e);
      setError(typeof e === "string" ? e : "Failed to start download");
    }
    refreshFiles();
  }, [refreshFiles]);

  // Handle retract
  const handleRetract = useCallback(async (fileId: string) => {
    try {
      await invoke("wormhole_retract", { fileId });
    } catch (e) {
      console.error("[Wormhole] Failed to retract:", e);
    }
    refreshFiles();
    refreshPendingCount();
  }, [refreshFiles, refreshPendingCount]);

  // Handle cancel
  const handleCancel = useCallback(async (fileId: string) => {
    try {
      await invoke("wormhole_cancel_download", { fileId });
    } catch (e) {
      console.error("[Wormhole] Failed to cancel:", e);
    }
    refreshFiles();
  }, [refreshFiles]);

  // Handle clear completed
  const handleClearCompleted = useCallback(async () => {
    try {
      await invoke("wormhole_clear_completed");
    } catch (e) {
      console.error("[Wormhole] Failed to clear:", e);
    }
    refreshFiles();
  }, [refreshFiles]);

  // Window drag handler for macOS
  const handleWindowDragStart = useCallback((event: ReactMouseEvent<HTMLElement>) => {
    if (!isMacPlatform || event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (target.closest("button, input, [data-no-drag]")) return;
    void getCurrentWindow().startDragging();
  }, []);

  // Separate files by category
  const localFiles = files.filter(f => f.is_local);
  const remoteFiles = files.filter(f => !f.is_local);
  const pendingRemote = remoteFiles.filter(f => f.status === "pending" || f.status === "available");
  const activeTransfers = files.filter(f => f.status === "uploading" || f.status === "downloading");
  const sharedLocal = localFiles.filter(
    f => f.status === "pending" || f.status === "available" || f.status === "uploading"
  );
  const completedFiles = files.filter(
    f => f.status === "completed" || f.status === "failed" || f.status === "expired" || f.status === "cancelled"
  );

  return (
    <div className="wormhole-root">
      {/* Header */}
      <header className="wormhole-header" onMouseDown={handleWindowDragStart}>
        {isMacPlatform && <div className="wormhole-header-spacer" />}
        <h1>Wormhole</h1>
        <div className="wormhole-header-actions">
          <button 
            className="wormhole-header-btn" 
            onClick={() => { refreshFiles(); refreshPendingCount(); }}
            title="Refresh"
          >
            <RefreshCw size={14} />
          </button>
          {completedFiles.length > 0 && (
            <button 
              className="wormhole-header-btn" 
              onClick={handleClearCompleted}
              title="Clear completed"
            >
              <Trash2 size={14} />
            </button>
          )}
        </div>
      </header>

      {/* Content */}
      <main className="wormhole-content">
        {/* Drop Zone */}
        <DropZone 
          onFilesDropped={handleFilesDropped}
          onSelectFiles={handleSelectFiles}
          isDragActive={isDragActive}
          setIsDragActive={setIsDragActive}
        />

        {/* Error message */}
        {error && (
          <div className="wormhole-error">
            <AlertCircle size={14} />
            <span>{error}</span>
            <button onClick={() => setError(null)}>
              <X size={12} />
            </button>
          </div>
        )}

        {/* Available for download (remote pending) */}
        {pendingRemote.length > 0 && (
          <section className="wormhole-section">
            <h2>
              <Download size={14} />
              Available for Download
              <span className="wormhole-section-badge">{pendingCount}</span>
            </h2>
            <div className="wormhole-file-list">
              {pendingRemote.map(file => (
                <FileCard
                  key={file.id}
                  file={file}
                  progress={progress.get(file.id)}
                  onDownload={() => handleDownload(file.id)}
                  onRetract={() => handleRetract(file.id)}
                  onCancel={() => handleCancel(file.id)}
                />
              ))}
            </div>
          </section>
        )}

        {/* Active transfers */}
        {activeTransfers.length > 0 && (
          <section className="wormhole-section">
            <h2>
              <RefreshCw size={14} className="animate-spin" />
              Transferring
            </h2>
            <div className="wormhole-file-list">
              {activeTransfers.map(file => (
                <FileCard
                  key={file.id}
                  file={file}
                  progress={progress.get(file.id)}
                  onDownload={() => handleDownload(file.id)}
                  onRetract={() => handleRetract(file.id)}
                  onCancel={() => handleCancel(file.id)}
                />
              ))}
            </div>
          </section>
        )}

        {/* Your shared files (local pending) */}
        {sharedLocal.length > 0 && (
          <section className="wormhole-section">
            <h2>
              <Upload size={14} />
              Your Shared Files
            </h2>
            <div className="wormhole-file-list">
              {sharedLocal.map(file => (
                <FileCard
                  key={file.id}
                  file={file}
                  progress={progress.get(file.id)}
                  onDownload={() => handleDownload(file.id)}
                  onRetract={() => handleRetract(file.id)}
                  onCancel={() => handleCancel(file.id)}
                />
              ))}
            </div>
          </section>
        )}

        {/* Completed / Failed / Expired */}
        {completedFiles.length > 0 && (
          <section className="wormhole-section">
            <h2>
              <Check size={14} />
              History
            </h2>
            <div className="wormhole-file-list">
              {completedFiles.map(file => (
                <FileCard
                  key={file.id}
                  file={file}
                  progress={progress.get(file.id)}
                  onDownload={() => handleDownload(file.id)}
                  onRetract={() => handleRetract(file.id)}
                  onCancel={() => handleCancel(file.id)}
                />
              ))}
            </div>
          </section>
        )}

        {/* Empty state */}
        {files.length === 0 && !error && (
          <div className="wormhole-empty">
            <span>No files yet</span>
            <span className="wormhole-empty-hint">Drop files above to share with synced devices</span>
          </div>
        )}
      </main>
    </div>
  );
}
