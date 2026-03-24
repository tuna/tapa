"""Module header template for TAPAS GraphIR exporter."""

from tapa.verilog.graphir_exporter.assets.templates.includes.header_parameters import (
    HEADER_PARAMETERS,
)
from tapa.verilog.graphir_exporter.assets.templates.includes.header_ports import (
    HEADER_PORTS,
)

MODULE_HEADER = f"""
module {{{{ name -}}}}

{{%- if parameters|length > 0 -%}}
{{# #}} #(
{{# #}}    {{# #}}
    {{%- filter indent(width=4) -%}}
    {HEADER_PARAMETERS}
    {{%- endfilter -%}}{{# #}}
) {{%- endif -%}}

{{# #}} (
{{%- if ports|length > 0 -%}}{{# #}}
    {{# #}}{{%- filter indent(width=4) -%}}
    {HEADER_PORTS}
    {{%- endfilter -%}}{{# #}}
{{# #}}{{%- endif -%}}
);
"""
