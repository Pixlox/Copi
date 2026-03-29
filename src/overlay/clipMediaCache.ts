export interface ClipIconData {
  thumbnail: string | null;
  app_icon: string | null;
}

const clipIconCache = new Map<number, ClipIconData>();
const imagePreviewCache = new Map<number, string>();

const CLIP_ICON_CACHE_MAX = 1000;
const IMAGE_PREVIEW_CACHE_MAX = 200;

function trimOldest<T>(cache: Map<number, T>, maxSize: number) {
  while (cache.size > maxSize) {
    const oldestKey = cache.keys().next().value;
    if (oldestKey === undefined) {
      break;
    }
    cache.delete(oldestKey);
  }
}

export function hasClipIconData(clipId: number): boolean {
  return clipIconCache.has(clipId);
}

export function getClipIconData(clipId: number): ClipIconData | undefined {
  return clipIconCache.get(clipId);
}

export function setClipIconData(clipId: number, data: ClipIconData) {
  if (clipIconCache.has(clipId)) {
    clipIconCache.delete(clipId);
  }
  clipIconCache.set(clipId, data);
  trimOldest(clipIconCache, CLIP_ICON_CACHE_MAX);
}

export function getImagePreviewData(clipId: number): string | undefined {
  return imagePreviewCache.get(clipId);
}

export function setImagePreviewData(clipId: number, preview: string) {
  if (imagePreviewCache.has(clipId)) {
    imagePreviewCache.delete(clipId);
  }
  imagePreviewCache.set(clipId, preview);
  trimOldest(imagePreviewCache, IMAGE_PREVIEW_CACHE_MAX);
}
