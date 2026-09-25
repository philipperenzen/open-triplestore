// A Leaflet layer over OpenFreeMap's vector tiles, drawn in the browser (see
// ofmRaster.ts): the previews' keyless street map.

import L from './leafletIcons';
import { OFM_CREDIT_HTML, renderOfmTile, type Palette } from './ofmRaster';

/** A GridLayer whose tiles are painted from OpenFreeMap in `palette`'s colours. */
export function ofmLeafletLayer(palette: Palette): L.GridLayer {
  const Layer = L.GridLayer.extend({
    createTile(coords: L.Coords, done: L.DoneCallback) {
      const size = (this as L.GridLayer).getTileSize();
      const ratio = Math.min(2, Math.max(1, window.devicePixelRatio || 1));
      const tile = document.createElement('canvas');
      tile.width = size.x * ratio;
      tile.height = size.y * ratio;
      tile.style.width = `${size.x}px`;
      tile.style.height = `${size.y}px`;
      renderOfmTile(coords.z, coords.x, coords.y, tile.width, palette)
        .then((painted) => {
          tile.getContext('2d')?.drawImage(painted, 0, 0);
          done(undefined, tile);
        })
        .catch((e) => done(e instanceof Error ? e : new Error(String(e)), tile));
      return tile;
    },
  });
  return new Layer({ maxZoom: 19, attribution: OFM_CREDIT_HTML });
}
