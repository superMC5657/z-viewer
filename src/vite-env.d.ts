/// <reference types="vite/client" />

interface ImageDecodeResult {
  image: VideoFrame;
  complete: boolean;
}

interface ImageDecodeOptions {
  frameIndex?: number;
  completeFramesOnly?: boolean;
}

interface ImageTrack {
  readonly frameCount: number;
  readonly repetitionCount: number;
  readonly animated: boolean;
  selected: boolean;
}

interface ImageTrackList {
  readonly ready: Promise<void>;
  readonly length: number;
  readonly selectedTrack: ImageTrack | null;
}

interface ImageDecoderInit {
  type: string;
  data: ReadableStream | BufferSource;
  premultiplyAlpha?: "none" | "premultiply" | "default";
  colorSpaceConversion?: "none" | "default";
  desiredWidth?: number;
  desiredHeight?: number;
  preferAnimation?: boolean;
}

declare class ImageDecoder {
  constructor(init: ImageDecoderInit);
  readonly type: string;
  readonly complete: boolean;
  readonly completed: Promise<void>;
  readonly tracks: ImageTrackList;
  decode(options?: ImageDecodeOptions): Promise<ImageDecodeResult>;
  reset(): void;
  close(): void;
  static isTypeSupported(type: string): Promise<boolean>;
}
