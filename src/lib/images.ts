import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";

// Pictures for the model, sanitized by the backend before anything else sees them.

/** Mirrors `domain::image::ImagePart`: a picture after sanitizing, `data` base64. */
export type ImagePart = { mediaType: "image/png" | "image/jpeg"; data: string; width: number; height: number };

/**
 * Sanitizes a pasted or dropped picture: decoded and encoded again, with no
 * metadata, PNG or JPEG only. The bytes go as the raw request body. Rejects
 * with a sentence the composer can show as is.
 */
export async function prepareImage(bytes: Uint8Array): Promise<ImagePart> {
  requireBackend();
  return invoke<ImagePart>("image_prepare", bytes);
}

/**
 * Sanitizes a file dropped on the window, by its path: the backend reads it,
 * size first, and answers with a picture encoded from its pixels — or a
 * sentence saying why not, for a file that is not a PNG or JPEG.
 */
export async function prepareImageFile(path: string): Promise<ImagePart> {
  requireBackend();
  return invoke<ImagePart>("image_prepare_file", { path });
}

/** The picture as an `<img src>`: a `data:` URL, which the window's CSP allows. */
export const imageUrl = (image: ImagePart) => `data:${image.mediaType};base64,${image.data}`;
