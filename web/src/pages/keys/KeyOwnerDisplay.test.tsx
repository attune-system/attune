import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OwnerType } from "@/api";
import KeyOwnerDisplay from "./KeyOwnerDisplay";

describe("KeyOwnerDisplay", () => {
  it.each([
    [OwnerType.PACK, "Pack", "core"],
    [OwnerType.ACTION, "Action", "core.echo"],
    [OwnerType.SENSOR, "Sensor", "core.timer"],
    [OwnerType.IDENTITY, "Identity", "alice@example.com"],
  ])("presents %s scope with its owner ref", (ownerType, label, ownerRef) => {
    render(<KeyOwnerDisplay ownerType={ownerType} ownerRef={ownerRef} />);

    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.getByText(ownerRef)).toBeInTheDocument();
  });

  it("presents a system key without repeating system as an owner ref", () => {
    render(<KeyOwnerDisplay ownerType={OwnerType.SYSTEM} ownerRef="system" />);

    expect(screen.getAllByText("System")).toHaveLength(1);
    expect(screen.queryByText("system")).not.toBeInTheDocument();
  });

  it("does not invent an owner reference when one is unavailable", () => {
    render(<KeyOwnerDisplay ownerType={OwnerType.IDENTITY} ownerRef={null} />);

    expect(screen.getByText("Identity")).toBeInTheDocument();
    expect(screen.queryByText("-")).not.toBeInTheDocument();
  });
});
