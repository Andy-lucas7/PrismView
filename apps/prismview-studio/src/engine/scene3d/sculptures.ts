import {
  BoxGeometry,
  Color,
  Group,
  Mesh,
  MeshPhysicalMaterial,
  MeshStandardMaterial,
  OctahedronGeometry,
  PlaneGeometry,
  SphereGeometry,
  TorusGeometry,
} from "three";
import type { Scene3DShowcaseVariant } from "../../types/prism";

// Built-in procedural "showcase" sculptures (used when a room object has no .glb
// or when a .glb fails to load).

export interface DefaultSculptureRefs {
  group: Group;
  tick: (t: number) => void;
}

export function buildShowcaseSculpture(variant: Scene3DShowcaseVariant = "MONOLITH"): DefaultSculptureRefs {
  const obsidianMat = new MeshPhysicalMaterial({
    name: "ObsidianCore",
    color: new Color("#15171e"),
    metalness: 0.88,
    roughness: 0.14,
    clearcoat: 1.0,
    clearcoatRoughness: 0.08,
    reflectivity: 1.0,
  });

  const chromeMat = new MeshStandardMaterial({
    name: "TitaniumRing",
    color: new Color("#d8dde8"),
    metalness: 0.96,
    roughness: 0.16,
  });

  const matteDarkMat = new MeshStandardMaterial({
    name: "GraphiteRing",
    color: new Color("#232630"),
    metalness: 0.7,
    roughness: 0.32,
  });

  if (variant === "GYROSCOPE") {
    const group = new Group();
    group.name = "PrismView_Solar_Gyroscope";

    const core = new Mesh(new SphereGeometry(0.62, 40, 28), obsidianMat);
    core.name = "AstroCore";
    group.add(core);

    const innerRing = new Mesh(new TorusGeometry(1.06, 0.026, 20, 96), chromeMat);
    innerRing.name = "InnerGimbal";
    group.add(innerRing);

    const midRing = new Mesh(new TorusGeometry(1.38, 0.022, 20, 96), matteDarkMat);
    midRing.name = "MidGimbal";
    midRing.rotation.set(Math.PI * 0.42, 0.3, 0);
    group.add(midRing);

    const outerRing = new Mesh(new TorusGeometry(1.7, 0.016, 16, 96), chromeMat);
    outerRing.name = "OuterGimbal";
    outerRing.rotation.set(-Math.PI * 0.25, Math.PI * 0.2, 0);
    group.add(outerRing);

    const markers = new Group();
    markers.name = "SolarMarkers";
    const markerGeo = new SphereGeometry(0.055, 16, 12);
    for (let i = 0; i < 4; i++) {
      const a = (i / 4) * Math.PI * 2;
      const m = new Mesh(markerGeo, i % 2 === 0 ? chromeMat : obsidianMat);
      m.name = `Marker_${i + 1}`;
      m.position.set(Math.cos(a) * 1.7, Math.sin(a) * 1.7, 0);
      markers.add(m);
    }
    outerRing.add(markers);

    return {
      group,
      tick: (t) => {
        core.position.y = Math.sin(t * 1.1) * 0.05;
        innerRing.rotation.x = t * 0.65;
        innerRing.rotation.y = t * 0.4;
        midRing.rotation.x = Math.PI * 0.42 + t * 0.32;
        midRing.rotation.z = -t * 0.45;
        outerRing.rotation.y = Math.PI * 0.2 + t * 0.22;
      },
    };
  }

  if (variant === "KINETIC_SLAB") {
    const group = new Group();
    group.name = "PrismView_Kinetic_Slab";

    const frameGroup = new Group();
    frameGroup.name = "KineticFrame";
    group.add(frameGroup);

    const body = new Mesh(new BoxGeometry(1.86, 1.86, 0.14), obsidianMat);
    body.name = "SlabBody";
    frameGroup.add(body);

    const accentMat = new MeshStandardMaterial({
      name: "KineticAccent",
      color: new Color("#2c3242"),
      emissive: new Color("#9fb6ff"),
      emissiveIntensity: 0.35,
      metalness: 0.85,
      roughness: 0.22,
    });
    const bezel = new Mesh(new BoxGeometry(1.94, 1.94, 0.08), accentMat);
    bezel.name = "AccentBezel";
    bezel.position.z = -0.02;
    frameGroup.add(bezel);

    const screenMat = new MeshStandardMaterial({
      name: "DisplayScreen",
      color: new Color("#1c202a"),
      emissive: new Color("#181c26"),
      emissiveIntensity: 0.4,
      metalness: 0.12,
      roughness: 0.25,
    });
    const screen = new Mesh(new PlaneGeometry(1.66, 1.66), screenMat);
    screen.name = "DisplayScreen";
    screen.position.z = 0.076;
    frameGroup.add(screen);

    const halo = new Mesh(new TorusGeometry(1.68, 0.02, 20, 96), chromeMat);
    halo.name = "PulseHalo";
    halo.position.z = -0.22;
    group.add(halo);

    const backRing = new Mesh(new TorusGeometry(1.92, 0.012, 16, 96), matteDarkMat);
    backRing.name = "OuterHalo";
    backRing.position.z = -0.35;
    group.add(backRing);

    return {
      group,
      tick: (t) => {
        frameGroup.position.y = Math.sin(t * 1.2) * 0.06;
        halo.rotation.z = t * 0.4;
        backRing.rotation.z = -t * 0.25;
      },
    };
  }

  const group = new Group();
  group.name = "PrismView_Obsidian_Monolith";

  const coreGeo = new OctahedronGeometry(0.86, 0);
  coreGeo.scale(0.86, 1.24, 0.86);
  const core = new Mesh(coreGeo, obsidianMat);
  core.name = "PrismCore";
  group.add(core);

  const ringAGeo = new TorusGeometry(1.38, 0.022, 20, 96);
  const ringA = new Mesh(ringAGeo, chromeMat);
  ringA.name = "OrbitalRingA";
  ringA.rotation.set(Math.PI * 0.36, Math.PI * 0.12, 0);
  group.add(ringA);

  const ringBGeo = new TorusGeometry(1.66, 0.014, 16, 96);
  const ringB = new Mesh(ringBGeo, matteDarkMat);
  ringB.name = "OrbitalRingB";
  ringB.rotation.set(-Math.PI * 0.28, Math.PI * 0.42, 0.2);
  group.add(ringB);

  const satellites = new Group();
  satellites.name = "Satellites";
  const satGeo = new BoxGeometry(0.11, 0.11, 0.11);
  for (let i = 0; i < 6; i++) {
    const angle = (i / 6) * Math.PI * 2;
    const radius = 1.38 + (i % 2) * 0.28;
    const m = new Mesh(satGeo, i % 2 === 0 ? chromeMat : obsidianMat);
    m.name = `Node_${i + 1}`;
    m.position.set(Math.cos(angle) * radius, Math.sin(angle * 2) * 0.32, Math.sin(angle) * radius);
    m.rotation.set(angle, angle * 0.7, 0.4);
    satellites.add(m);
  }
  group.add(satellites);

  return {
    group,
    tick: (t) => {
      core.rotation.y = t * 0.7;
      core.position.y = Math.sin(t * 1.4) * 0.08;
      ringA.rotation.z = t * 0.5;
      ringB.rotation.x = -Math.PI * 0.28 + Math.sin(t * 0.8) * 0.14;
      ringB.rotation.z = -t * 0.35;
      satellites.rotation.y = -t * 0.45;
    },
  };
}
