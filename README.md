# Monster Memory Engine 👹

Motor experimental de gestión de memoria escrito en Rust que explora la **deduplicación entre espacios de trabajo** y la **compresión en memoria de recursos web**.

El proyecto incluye actualmente:

- Motor de memoria basado en bloques de 64 KiB.
- Deduplicación mediante BLAKE3.
- Verificación byte a byte antes de reutilizar bloques.
- Seguimiento de referencias.
- Estados lógicos HOT, WARM y COLD.
- Almacenamiento RAW.
- Compresión LZ4.
- Soporte para compresión Zstd.
- Detección de bloques que no se benefician de la compresión.
- Integración experimental con CEF 154.
- Captura en vivo de respuestas HTTP mediante `ResponseFilter`.
- Métricas en tiempo real de bloques lógicos, únicos, duplicados y memoria física administrada.

> [!IMPORTANT]
> Monster Memory Engine sigue siendo una prueba de concepto.
>
> El proyecto ya incluye una integración experimental con CEF 154 escrita en Rust. Las respuestas HTTP se interceptan mediante un `ResponseFilter` transparente y se envían al motor de memoria.
>
> Las métricas físicas mostradas por Memora corresponden únicamente a los recursos almacenados por su gestor. No representan el consumo total de memoria de Chromium o CEF.

Este es un proyecto experimental **dirigido por una persona y asistido por inteligencia artificial**.

La inteligencia artificial se ha utilizado durante:

- Exploración de arquitectura.
- Creación de prototipos en Rust.
- Refactorización.
- Análisis de resultados.
- Diseño de pruebas.
- Depuración.
- Documentación.

Las decisiones técnicas, ejecución, pruebas y validación siguen siendo responsabilidad humana.

---

## Objetivo

Los navegadores modernos aíslan pestañas, sitios y procesos de renderizado por motivos de seguridad, estabilidad y tolerancia a fallos.

Sin embargo, diferentes pestañas o espacios de trabajo pueden cargar recursos idénticos:

- JavaScript.
- CSS.
- HTML.
- JSON.
- Imágenes.
- Fuentes.
- WebAssembly.
- Otros recursos inmutables.

Monster Memory Engine investiga dos preguntas principales:

> ¿Pueden diferentes espacios de trabajo reutilizar físicamente recursos idénticos sin perder su aislamiento lógico?

> ¿Pueden los recursos inactivos mantenerse comprimidos en memoria y restaurarse cuando vuelvan a utilizarse?

Modelo conceptual:

```text
Workspace A ──────────────┐
                          │
Workspace B ──────────────┼──► Shared Resource Store
                          │         │
Workspace C ──────────────┘         ├── BLAKE3
                                    ├── deduplicación
                                    ├── referencias
                                    └── compresión
```

El estado privado continúa aislado:

```text
Workspace A
├── sesión
├── estado privado
├── runtime
└── referencias ──────────┐

Workspace B               │
├── sesión                ├──► bloques físicos compartidos
├── estado privado        │
├── runtime               │
└── referencias ──────────┘
```

---

# Modos de memoria

## RAM

```text
Deduplicación: activada
Compresión:    desactivada
Almacenamiento: RAW
```

Pensado para sistemas donde la capacidad de memoria no es una preocupación y se busca minimizar el trabajo adicional de CPU.

La deduplicación continúa activa.

## LZ4

```text
Deduplicación: activada
Compresión:    LZ4
```

Los bloques inactivos se comprimen cuando la compresión reduce realmente su tamaño.

El objetivo es reducir memoria manteniendo una restauración rápida.

## Zstd

```text
Deduplicación: activada
Compresión:    Zstd
```

Pensado para escenarios donde la reducción de memoria tiene mayor prioridad y se acepta un coste adicional de CPU.

El nivel de compresión es configurable.

Ejemplo del prototipo original:

```powershell
cargo run --release -- zstd captures 5
```

---

# Arquitectura del motor

```text
Recurso web
    │
    ▼
bloques de 64 KiB
    │
    ▼
hash BLAKE3
    │
    ▼
búsqueda de candidatos
    │
    ▼
verificación byte a byte
    │
    ├── idéntico ──► reutilizar bloque
    │
    └── diferente ─► crear bloque
                          │
                          ▼
                    referencias
                          │
              ┌───────────┼───────────┐
              ▼           ▼           ▼
             HOT         WARM        COLD
              │           │           │
              └───────────┴───────────┘
                          │
                          ▼
                  modo seleccionado
                          │
              ┌───────────┼───────────┐
              ▼           ▼           ▼
             RAW         LZ4         Zstd
```

La temperatura lógica y el formato físico son conceptos independientes.

Un bloque puede tener:

```text
Temperatura: COLD
Almacenamiento: RAW
```

Esto ocurre cuando:

- La compresión está desactivada.
- El bloque no obtiene ahorro al comprimirse.
- El algoritmo ya fue probado y rechazado para ese bloque.

---

# Deduplicación

Cada recurso se divide en bloques de 64 KiB.

```text
bloque
  │
  ▼
BLAKE3
  │
  ▼
índice de candidatos
  │
  ▼
comparación de tamaño
  │
  ▼
restauración verificada
  │
  ▼
comparación byte a byte
```

El hash no se considera prueba suficiente de igualdad.

Dos bloques solo se deduplican cuando:

1. El hash BLAKE3 coincide.
2. El tamaño original coincide.
3. El contenido restaurado coincide byte a byte.

Cuando se detecta un duplicado:

- No se crea un nuevo bloque físico.
- Se incrementa el contador de referencias.
- Se registra la referencia del espacio de trabajo.
- Se incrementa el contador de bloques duplicados.

---

# Modelo térmico

Los bloques tienen una temperatura lógica:

```text
HOT
 │
 │ inactividad
 ▼
WARM
 │
 │ más inactividad
 ▼
COLD
```

Valores experimentales actuales:

```text
WARM después de:       50 ticks
COLD después de:      200 ticks
Ciclo de aging:        25 recursos
```

Los bloques nuevos entran como:

```text
Temperatura: HOT
Almacenamiento: RAW
```

Cuando envejecen:

- En modo RAM permanecen RAW.
- En modo LZ4 intentan comprimirse con LZ4.
- En modo Zstd intentan comprimirse con Zstd.
- Si la compresión no reduce el tamaño, permanecen RAW.

---

# Aprendizaje de compresión

No todos los recursos se benefician de la compresión.

Ejemplos frecuentes:

- Imágenes ya comprimidas.
- Audio.
- Vídeo.
- Fuentes.
- Binarios.
- Algunos recursos cifrados o empaquetados.

Proceso utilizado:

```text
bloque RAW
    │
    ▼
comprimir
    │
    ▼
¿comprimido < original?
    │
    ├── sí ──► almacenar comprimido
    │
    └── no ──► conservar RAW
                    │
                    ▼
          recordar rechazo
```

Los bloques son inmutables en el modelo actual. Si un bloque no se beneficia de LZ4 o Zstd, el motor recuerda el resultado y evita gastar CPU repitiendo la misma prueba.

---

# Integración en vivo con CEF

El repositorio contiene un navegador experimental basado en CEF 154 dentro de:

```text
browser/
```

Flujo implementado:

```text
respuesta de red de CEF
        │
        ▼
ResourceRequestHandler
        │
        ▼
ResponseFilter transparente
        │
        ├── reenvía los bytes a Chromium
        │
        └── captura una copia para Memora
                    │
                    ▼
              respuesta completa
                    │
                    ▼
              bloques de 64 KiB
                    │
                    ▼
                 BLAKE3
                    │
                    ▼
              deduplicación
                    │
                    ▼
             envejecimiento
                    │
                    ▼
              RAW / LZ4 / Zstd
```

El filtro es pasante.

Chromium recibe los mismos bytes mientras Memora conserva una copia para procesarla.

Actualmente se almacenan únicamente respuestas que:

- Finalizan correctamente.
- Tienen un cuerpo no vacío.
- Fueron capturadas por el `ResponseFilter`.

Las respuestas vacías y las cargas fallidas no se insertan en el gestor.

---

## Componentes principales de la integración

### `ResourceRequestHandler`

Permite observar:

- URL solicitada.
- Inicio de la carga.
- Respuesta HTTP.
- MIME.
- Código HTTP.
- Finalización de la carga.

### `ResponseFilter`

Permite:

- Recibir los fragmentos de respuesta.
- Copiarlos sin modificación hacia Chromium.
- Acumular una copia completa para Memora.
- Informar correctamente los bytes consumidos y producidos.

### `MemoryResource`

Agrupa:

```rust
workspace
url
mime_type
status
body
```

### `MemoryManager`

Procesa el cuerpo capturado:

```text
body
  │
  ▼
chunks de 64 KiB
  │
  ▼
BLAKE3
  │
  ▼
deduplicación
  │
  ▼
almacenamiento físico
```

---

# Alcance actual del navegador

## Implementado

- [x] CEF 154 integrado desde Rust.
- [x] Ventana básica del navegador.
- [x] Navegación por URL.
- [x] Interceptación de recursos en vivo.
- [x] Captura de respuestas mediante `ResponseFilter`.
- [x] Copia transparente sin modificar los bytes de Chromium.
- [x] Captura de URL, MIME, estado HTTP y cuerpo.
- [x] Inserción de recursos completos en el gestor.
- [x] División en bloques de 64 KiB.
- [x] BLAKE3.
- [x] Verificación byte a byte.
- [x] Deduplicación real.
- [x] Referencias lógicas.
- [x] Modelo HOT, WARM y COLD.
- [x] Integración con LZ4.
- [x] Soporte del motor para Zstd.
- [x] Métricas en vivo.
- [x] Pruebas con navegación real.
- [x] Pruebas con reproducción de vídeo.

## Pendiente

- [ ] Múltiples espacios de trabajo aislados.
- [ ] Configuración RAM/LZ4/Zstd desde la interfaz.
- [ ] Límite configurable de memoria.
- [ ] Política de expulsión.
- [ ] Liberación de bloques sin referencias.
- [ ] Asociación completa entre recursos y pestañas.
- [ ] Suspensión de pestañas inactivas.
- [ ] Liberación real del renderer de Chromium.
- [ ] Restauración de pestañas.
- [ ] Comparación formal con Chromium sin modificar.

---

# Resultado observado con tráfico real

Durante una prueba intensiva se observó:

```text
Resources: 2865
Blocks: 1675 unique / 3652 logical
Duplicates: 1977
Logical: 64.34 MiB
Physical: 23.91 MiB
```

Interpretación:

- `Resources`: respuestas completas insertadas.
- `Logical blocks`: total de referencias lógicas recibidas.
- `Unique blocks`: bloques físicos diferentes.
- `Duplicates`: bloques reutilizados.
- `Logical`: volumen total recibido por Memora.
- `Physical`: bytes físicos almacenados por el gestor después de deduplicar y comprimir.

En esa prueba:

```text
Bloques lógicos:   3652
Bloques únicos:    1675
Duplicados:        1977
```

Esto demuestra que una parte importante del tráfico estaba formada por contenido repetido.

> [!WARNING]
> `Physical: 23.91 MiB` no significa que Chromium utilizara únicamente 23.91 MiB de RAM.

Chromium continúa manteniendo por separado:

- Procesos renderer.
- Heap de JavaScript.
- Estado de V8.
- DOM de Blink.
- Imágenes decodificadas.
- Recursos GPU.
- Buffers de audio y vídeo.
- Cachés internas.
- Estado de pestañas.
- Memoria del proceso principal.
- Procesos auxiliares.

En una prueba agresiva, el conjunto de procesos CEF/Chromium llegó a consumir varios gigabytes mientras Memora administraba decenas de MiB.

Esto confirma que la captura de recursos HTTP es solo una parte del problema.

---

# Próxima fase: ciclo de vida de pestañas

La siguiente fase de investigación es conseguir que Chromium libere memoria de pestañas inactivas y utilice Memora como almacenamiento de respaldo.

Flujo conceptual:

```text
PESTAÑA ACTIVA
      │
      ▼
PESTAÑA INACTIVA
      │
      ▼
capturar estado recuperable
      │
      ▼
almacenar mediante Memora
      │
      ▼
suspender o descartar renderer
      │
      ▼
liberar memoria real de Chromium
      │
      ▼
usuario vuelve a la pestaña
      │
      ▼
restaurar o reconstruir
```

La parte clave es:

> Capturar y comprimir recursos HTTP no hace que Chromium libere automáticamente sus propias copias ni el estado interno de la pestaña.

Para reducir materialmente la RAM total será necesario investigar:

- Detección de pestañas inactivas.
- Freeze del renderer.
- Descarte del renderer.
- Serialización de estado recuperable.
- Restauración de navegación.
- Reconstrucción de la pestaña.
- Restauración de formularios y scroll.
- Manejo de sesiones.
- Política de activación y descarte.
- Medición de latencia de restauración.

---

# Benchmark original con HAR

El prototipo original utiliza recursos capturados mediante archivos HAR.

Dataset utilizado:

```text
Contenido lógico:           91.25 MiB
Único tras deduplicación:   58.58 MiB
Ahorro por deduplicación:   32.67 MiB
Relación de deduplicación:  35.81 %
Bloques lógicos:            1751
Bloques únicos:             1143
Bloques duplicados:          608
Tamaño de bloque:           64 KiB
```

## Comparación de modos

| Modo         | Memoria física final | Ahorro total |
| ------------ | -------------------: | -----------: |
| RAM          |            58.58 MiB |      35.81 % |
| LZ4          |            25.71 MiB |      71.83 % |
| Zstd nivel 3 |            20.35 MiB |      77.70 % |

Estos resultados corresponden al conjunto de recursos administrado por el prototipo.

No significan que Chromium utilice un 77 % menos de RAM.

## Latencia observada

```text
Acceso RAW:        ~35-38 ns
Restauración LZ4:  ~34 µs
Restauración Zstd: ~95 µs
```

El acceso RAW mide la ruta directa de la simulación. No es una prueba de ancho de banda de memoria del sistema.

---

# Ejecución del motor original

## Requisitos

- Rust.
- Cargo.
- En Windows, toolchain MSVC.
- Herramientas de compilación de C++ correspondientes.

Compilar:

```powershell
cargo build --release
```

## Modo RAM

```powershell
cargo run --release -- ram
```

## Modo LZ4

```powershell
cargo run --release -- lz4
```

## Modo Zstd

```powershell
cargo run --release -- zstd
```

Nivel personalizado:

```powershell
cargo run --release -- zstd captures 5
```

---

# Ejecución del navegador CEF

Desde:

```text
C:\temp\rust\browser
```

Ejecutar:

```powershell
cargo run --bin memora-browser
```

La configuración actual del navegador utiliza LZ4 como modo predeterminado.

Ejemplo de métricas:

```text
[Memora] Stored |
Resources: 164 |
Blocks: 362 unique / 394 logical |
Duplicates: 32 |
Logical: 16.55 MiB |
Physical: 12.03 MiB
```

---

# Integridad

Cada bloque conserva su hash BLAKE3 original.

Después de restaurar un bloque:

```text
bloque almacenado
       │
       ▼
descomprimir si corresponde
       │
       ▼
bytes restaurados
       │
       ▼
BLAKE3
       │
       ▼
comparar con hash original
```

Si el hash no coincide, la restauración se considera inválida.

---

# Privacidad

Las capturas HAR y los logs del navegador pueden contener:

- URL privadas.
- Parámetros de solicitud.
- Tokens.
- Datos de autenticación.
- Respuestas de API.
- Hostnames internos.
- Información personal.
- Recursos corporativos.

No deben subirse capturas privadas al repositorio.

Configuración recomendada para `.gitignore`:

```gitignore
/target/
/captures/
*.har

.vscode/
.idea/
.DS_Store
Thumbs.db
```

Antes de publicar cambios:

```powershell
git status
git diff --cached
```

---

# Seguridad

Compartir almacenamiento físico no debe implicar compartir permisos lógicos.

```text
Almacenamiento físico
        │
        └── puede compartirse

Propiedad lógica
        │
        └── debe permanecer aislada
```

La integración futura debe respetar:

- Same-origin policy.
- Site isolation.
- Sandbox de Chromium.
- Aislamiento de procesos.
- Permisos por origen.
- Sesiones independientes.
- Datos privados por espacio de trabajo.

---

# Limitaciones actuales

El proyecto combina una simulación basada en HAR con un navegador CEF experimental.

Actualmente no:

- Sustituye el asignador de memoria de Chromium.
- Comprime páginas arbitrarias del heap de V8.
- Deduplica estado mutable del DOM.
- Controla memoria GPU.
- Implementa paginación virtual transparente.
- Implementa zram real a nivel del sistema operativo.
- Implementa páginas copy-on-write.
- Mide automáticamente el working set completo del navegador.
- Suspende automáticamente pestañas inactivas.
- Restaura el estado completo de una pestaña.
- Proporciona garantías de seguridad para producción.

---

# Roadmap

## Etapa 1: motor de memoria

- [x] Almacenamiento por bloques.
- [x] Deduplicación BLAKE3.
- [x] Verificación byte a byte.
- [x] Referencias por espacio de trabajo.
- [x] Estados HOT, WARM y COLD.
- [x] Compresión LZ4.
- [x] Compresión Zstd.
- [x] Detección de utilidad de compresión.
- [x] Aprendizaje de resultados.
- [x] Descompresión bajo demanda.
- [x] Verificación de integridad.
- [x] Modos RAM, LZ4 y Zstd.
- [x] Simulación dinámica.

## Etapa 2: prototipo de navegador

- [x] Integrar CEF desde Rust.
- [x] Crear ventana básica.
- [x] Navegación por URL.
- [ ] Espacios de trabajo aislados.
- [x] Interceptación de recursos en vivo.
- [x] Captura mediante `ResponseFilter`.
- [x] Conectar recursos vivos con el gestor.
- [ ] Selección de RAM/LZ4/Zstd desde configuración.

## Etapa 3: memoria de pestañas

- [ ] Medir working set total.
- [ ] Comparar con Chromium sin modificar.
- [ ] Asociar recursos con pestañas.
- [ ] Liberar referencias al cerrar pestañas.
- [ ] Detectar pestañas inactivas.
- [ ] Suspender o descartar renderers.
- [ ] Conservar estado recuperable.
- [ ] Restaurar pestañas.
- [ ] Medir latencia de restauración.
- [ ] Medir coste de CPU.
- [ ] Probar diferentes clases de procesador.

## Etapa 4: integración profunda

Si CEF proporciona control suficiente:

```text
continuar con CEF
```

Si se necesita control de:

- Blink.
- V8.
- Renderers.
- Allocators.
- GPU.
- Decoded image cache.
- Lifecycle interno.

entonces podría ser necesario investigar un fork de Chromium.

---

# Historial

```text
v0.1
Benchmark sintético de deduplicación y LZ4/Zstd

v0.2
Recursos reales JS, CSS, HTML, JSON, imágenes y fuentes

v0.3
Ingesta mediante HAR

v0.4
Almacén compartido y referencias por workspace

v0.5
Modelo HOT/WARM/COLD

v0.6
Simulación dinámica de accesos

v0.6.1
Aprendizaje de compresión y ruta HOT optimizada

v0.7
Modos RAM/LZ4/Zstd seleccionables
Separación entre temperatura y códec físico

v0.8
Integración experimental con CEF 154
Captura en vivo con ResourceRequestHandler
ResponseFilter transparente
Conexión con el motor de bloques
Deduplicación sobre tráfico real
Métricas de memoria administrada
```

---

# Estado del proyecto

**Experimental / Proof of Concept**

Actualmente el proyecto demuestra que:

- CEF puede capturar respuestas HTTP en vivo.
- El filtro puede reenviar los bytes sin impedir la navegación.
- Los cuerpos se pueden dividir en bloques.
- Los bloques se pueden deduplicar con BLAKE3.
- La deduplicación funciona sobre tráfico real.
- Los bloques pueden envejecer.
- LZ4 puede reducir el almacenamiento físico.
- Zstd está disponible como estrategia del motor.
- Los bloques incomprimibles pueden mantenerse RAW.
- Los bloques restaurados pueden verificarse.
- La memoria administrada por Memora puede medirse por separado.

El siguiente gran objetivo es investigar el ciclo de vida de pestañas inactivas para liberar memoria real de Chromium y restaurar la pestaña después.

---

# Descargo de responsabilidad

Monster Memory Engine es software experimental de investigación.

Los resultados dependen de:

- Tipo de carga.
- Recursos utilizados.
- Porcentaje de duplicación.
- Arquitectura del procesador.
- Modo de compresión.
- Nivel de Zstd.
- Tamaño del bloque.
- Patrones de acceso.
- Comportamiento de Chromium.
- Comportamiento de los sitios web.

Los resultados del gestor no deben interpretarse automáticamente como ahorro total de memoria del navegador.

---

# Licencia

Todavía no se ha seleccionado una licencia.

Antes de aceptar contribuciones externas o distribuir binarios deben revisarse:

- La licencia del proyecto.
- Las licencias de las dependencias.
- Las condiciones de redistribución de CEF.
- Las condiciones aplicables a Chromium.
