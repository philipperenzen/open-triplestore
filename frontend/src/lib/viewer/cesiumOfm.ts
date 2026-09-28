// A Cesium imagery provider over OpenFreeMap's vector tiles, drawn in the
// browser (see ofmRaster.ts): the globe's keyless street map.

import { OFM_CREDIT_HTML, renderOfmTile, type Palette } from './ofmRaster';

// Cesium is imported dynamically by the viewer, so its namespace (and the
// provider object Cesium consumes) arrive untyped here.
/**
 * An object implementing Cesium's `ImageryProvider` interface whose tiles are
 * canvases painted from OpenFreeMap tiles in `palette`'s colours.
 */
export function ofmImageryProvider(Cesium: any, palette: Palette): any {
  const tilingScheme = new Cesium.WebMercatorTilingScheme();
  return {
    tilingScheme,
    rectangle: tilingScheme.rectangle,
    tileWidth: 256,
    tileHeight: 256,
    minimumLevel: 0,
    maximumLevel: 19,
    tileDiscardPolicy: undefined,
    proxy: undefined,
    hasAlphaChannel: false,
    errorEvent: new Cesium.Event(),
    credit: new Cesium.Credit(OFM_CREDIT_HTML, true),
    getTileCredits: () => undefined,
    requestImage: (x: number, y: number, level: number) => renderOfmTile(level, x, y, 256, palette),
    pickFeatures: () => undefined,
  };
}
