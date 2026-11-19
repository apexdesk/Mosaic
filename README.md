# Mosaic

Tablero de referencias para artistas. Imágenes, vídeos, PDF y documentos de oficina, modelos 3D,
audio, texto y dibujo a mano alzada en un lienzo infinito. Hecho en Rust con
[egui](https://github.com/emilk/egui) y OpenGL. Funciona en Linux, Windows y macOS.

## Descarga

En [Releases](https://github.com/apexdesk/Mosaic/releases):

- **Linux:** `.deb`, `.rpm`, paquete de Arch, Flatpak o `.tar.gz` (x86_64 y aarch64)
- **Windows:** instalador `.msi`, `.msix`, zip portable o el `.exe` suelto (x64 y ARM64)
- **macOS:** `Mosaic.app` universal (Apple Silicon e Intel)

La app de macOS no está firmada con un certificado de Apple: la primera vez, clic derecho → Abrir.
El `.msix` usa un certificado autofirmado (`mosaic-msix.cer`) que hay que instalar antes.

Los vídeos necesitan `ffmpeg` instalado (y `ffplay` para el sonido).

## Compilar

```sh
cargo run --release -- [tablero.refs | imágenes… | carpetas…]
```

En Linux, `packaging/install.sh` lo compila e instala para el usuario actual (binario en
`~/.local/bin`, lanzador, icono y tipo de archivo `.refs`); `packaging/uninstall.sh` lo quita.

## Uso

Arrastra a la ventana imágenes, vídeos, documentos, modelos 3D o carpetas, también imágenes desde el
navegador. También se puede pegar con `Ctrl+V` o importar con `Ctrl+I`. Clic derecho abre el menú y
`F1` muestra los atajos.

Cualquier elemento se puede arrastrar fuera de la ventana a otra aplicación (Krita, Photoshop,
Blender, el gestor de archivos…) y se entrega el archivo original.

`Ctrl+Mayús+↑/↓` hace translúcida toda la ventana, útil para ponerla encima de otro programa y
calcar. `Ctrl+T` la deja siempre encima. En Wayland eso no está permitido, así que ofrece reiniciar
en modo XWayland; en GNOME también vale Alt+Espacio → «Siempre encima».

La interfaz está en español, inglés, francés, alemán, portugués, italiano, ruso, chino, japonés y
coreano.

| Tipo | Formatos | |
|---|---|---|
| Imagen | JPEG, PNG, GIF, WebP, BMP, TIFF, TGA, ICO | |
| Vídeo | MP4, MKV, MOV, WebM, AVI… | en bucle, pausa, salto, silencio |
| Documento | PDF, DOCX, XLSX, PPTX, ODT, ODS, ODP, RTF, DOC, XLS, PPT | pasar páginas o expandirlas todas |
| Modelo 3D | glTF/GLB, STL, OBJ, PLY, 3MF | doble clic y arrastrar para girar; `O` cambia la perspectiva |
| Audio | MP3, WAV, OGG, FLAC, M4A/AAC, ALAC, AIFF | reproductor con forma de onda y carátula |
| Texto | | herramienta `T`, con formato básico |
| Trazo | | herramienta `D` |
| Marco | | herramienta `K` o `Ctrl+G`: agrupa lo que contiene |

`Ctrl+clic` abre los enlaces de textos y PDF. `C` recorta imágenes, vídeos y páginas sin perder
nada. `Ctrl+L` emparenta la selección al primer elemento seleccionado.

### Atajos

| Atajo | Acción |
|---|---|
| `V` · `T` · `D` · `K` | Mover · texto · dibujar · marco |
| Rueda / pellizco | Zoom |
| Botón central, `Espacio` o `Alt` + arrastrar | Desplazar la vista |
| Esquinas · círculo superior | Escalar · girar (`Mayús`: pasos de 15°) |
| Doble clic | Encuadrar · reproducir vídeo · editar texto · girar modelo 3D |
| `F` · `Inicio` · `1` | Encuadrar selección · todo · tamaño real |
| `A` · `N` | Ordenar · igualar alturas |
| `Alt+←/→/↑/↓` · `Alt+H` · `Alt+V` | Alinear · centrar |
| `M` · `G` · `R` / `Mayús+R` · `Ctrl+R` | Espejo · grises · girar 90° · restablecer |
| `[` · `]` | Al fondo · al frente |
| `P` · `Mayús+M` | Reproducir/pausa · silenciar vídeos |
| `,` · `.` · `E` | Página anterior · siguiente · expandir todas |
| `Ctrl+C` · `Ctrl+V` · `Ctrl+D` · `Supr` | Copiar · pegar · duplicar · eliminar |
| `Ctrl+Z` · `Ctrl+Y` | Deshacer · rehacer |
| `Ctrl+O` · `Ctrl+S` · `Ctrl+Mayús+S` | Abrir · guardar · guardar como |
| `Mayús+G` · `Ctrl+Mayús+G` | Cuadrícula · tema blueprint |
| Flechas (`Mayús`: ×10) | Mover la selección |

## Formato `.refs`

Un solo archivo con todo dentro: los bytes originales de cada imagen, vídeo, documento o modelo,
sin recomprimir y sin duplicados, más una cabecera JSON con la vista, los textos y los trazos. Se
abre mapeado en memoria y se guarda de forma atómica.

## Rendimiento

Las imágenes se suben a la GPU por niveles de detalle y solo se carga el que hace falta para el
tamaño en pantalla, empezando por lo visible. Hay un límite de VRAM con LRU (1,5 GB,
`REFS_VRAM_MB` lo cambia). Los JPEG se decodifican ya reducidos y lo importado se mapea desde disco
en vez de ocupar RAM. Sin interacción no se repinta nada, así que en reposo no gasta CPU.

Buena parte de esto, el renderizador de documentos (`vendor/apex-view`), el visor 3D y el arrastre
entre aplicaciones vienen de [File Browser](https://github.com/apexdesk/File-Browser).

## Desarrollo

```sh
cargo test
REFS_SHOT=/tmp/c.png cargo run --release --features selftest -- ~/Imágenes   # captura automática
```

`vendor/egui-winit` es egui-winit con un parche para que `Ctrl+V` llegue a la app aunque el
portapapeles no tenga texto. Las releases se generan en GitHub Actions al subir una etiqueta `v*`.

## Licencia

[PolyForm Strict 1.0.0](LICENSE): el código se puede leer y usar con fines personales y no
comerciales, pero no redistribuir, modificar ni usar comercialmente. Es código disponible, no código
abierto. Las dependencias de terceros tienen sus propias licencias (`vendor/egui-winit` es MIT o
Apache-2.0).
