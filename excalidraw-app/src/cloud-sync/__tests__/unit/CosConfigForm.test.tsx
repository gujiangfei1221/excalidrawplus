import { fireEvent, render, screen } from "@testing-library/react";

import { CosConfigForm } from "../../components/CosConfigForm";

describe("CosConfigForm", () => {
  it("renders all COS fields", () => {
    render(<CosConfigForm onSubmit={async () => {}} />);

    expect(screen.getByLabelText("SecretId")).toBeInTheDocument();
    expect(screen.getByLabelText("SecretKey")).toBeInTheDocument();
    expect(screen.getByLabelText("Bucket")).toBeInTheDocument();
    expect(screen.getByLabelText("Region")).toBeInTheDocument();
  });

  it("blocks submission when fields are empty", () => {
    const onSubmit = vi.fn();
    render(<CosConfigForm onSubmit={onSubmit} />);

    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));

    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "请填写所有 COS 字段。",
    );
  });

  it("displays validation errors from the caller", () => {
    render(
      <CosConfigForm
        error="COS connection validation failed"
        onSubmit={async () => {}}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "COS connection validation failed",
    );
  });
});
