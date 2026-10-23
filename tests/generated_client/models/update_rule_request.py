from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.update_rule_request_action_params_type_0 import (
        UpdateRuleRequestActionParamsType0,
    )
    from ..models.update_rule_request_conditions_type_0 import (
        UpdateRuleRequestConditionsType0,
    )
    from ..models.update_rule_request_sensor_worker_affinity_type_0 import (
        UpdateRuleRequestSensorWorkerAffinityType0,
    )
    from ..models.update_rule_request_sensor_worker_selector_type_0 import (
        UpdateRuleRequestSensorWorkerSelectorType0,
    )
    from ..models.update_rule_request_sensor_worker_tolerations_type_0_item import (
        UpdateRuleRequestSensorWorkerTolerationsType0Item,
    )
    from ..models.update_rule_request_trigger_params_type_0 import (
        UpdateRuleRequestTriggerParamsType0,
    )


T = TypeVar("T", bound="UpdateRuleRequest")


@_attrs_define
class UpdateRuleRequest:
    """Request DTO for updating a rule

    Attributes:
        action_params (None | Unset | UpdateRuleRequestActionParamsType0): Parameters to pass to the action when rule is
            triggered
        action_ref (None | str | Unset): Action reference to execute when rule matches Example: slack.post_message.
        conditions (None | Unset | UpdateRuleRequestConditionsType0): Conditions for rule evaluation
        description (None | str | Unset): Rule description Example: Enhanced error notification with filtering.
        enabled (bool | None | Unset): Whether the rule is enabled Example: False.
        label (None | str | Unset): Human-readable label Example: Notify on Error (Updated).
        permission_set_refs (list[str] | None | Unset): Permission set refs to apply to executions created by this rule.
            Omit to
            keep the current value. Provide null to inherit the action default, or an
            empty array to force no API token. Example: ['core.agent_reader'].
        sensor_worker_affinity (None | Unset | UpdateRuleRequestSensorWorkerAffinityType0): Replacement sensor-worker
            affinity.
        sensor_worker_selector (None | Unset | UpdateRuleRequestSensorWorkerSelectorType0): Replacement sensor-worker
            selector.
        sensor_worker_tolerations (list[UpdateRuleRequestSensorWorkerTolerationsType0Item] | None | Unset): Replacement
            sensor-worker tolerations.
        trace_tag_template (None | str | Unset): Optional template used to resolve execution trace tags for this rule.
            Omit to keep current value. Provide null to clear. Example: {{ event.trigger }}.{{ event.id }}.
        trigger_params (None | Unset | UpdateRuleRequestTriggerParamsType0): Parameters for trigger configuration and
            event filtering
        trigger_ref (None | str | Unset): Trigger reference that activates this rule Example: system.error_event.
    """

    action_params: None | Unset | UpdateRuleRequestActionParamsType0 = UNSET
    action_ref: None | str | Unset = UNSET
    conditions: None | Unset | UpdateRuleRequestConditionsType0 = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    sensor_worker_affinity: (
        None | Unset | UpdateRuleRequestSensorWorkerAffinityType0
    ) = UNSET
    sensor_worker_selector: (
        None | Unset | UpdateRuleRequestSensorWorkerSelectorType0
    ) = UNSET
    sensor_worker_tolerations: (
        list[UpdateRuleRequestSensorWorkerTolerationsType0Item] | None | Unset
    ) = UNSET
    trace_tag_template: None | str | Unset = UNSET
    trigger_params: None | Unset | UpdateRuleRequestTriggerParamsType0 = UNSET
    trigger_ref: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_rule_request_action_params_type_0 import (
            UpdateRuleRequestActionParamsType0,
        )
        from ..models.update_rule_request_conditions_type_0 import (
            UpdateRuleRequestConditionsType0,
        )
        from ..models.update_rule_request_sensor_worker_affinity_type_0 import (
            UpdateRuleRequestSensorWorkerAffinityType0,
        )
        from ..models.update_rule_request_sensor_worker_selector_type_0 import (
            UpdateRuleRequestSensorWorkerSelectorType0,
        )
        from ..models.update_rule_request_trigger_params_type_0 import (
            UpdateRuleRequestTriggerParamsType0,
        )

        action_params: dict[str, Any] | None | Unset
        if isinstance(self.action_params, Unset):
            action_params = UNSET
        elif isinstance(self.action_params, UpdateRuleRequestActionParamsType0):
            action_params = self.action_params.to_dict()
        else:
            action_params = self.action_params

        action_ref: None | str | Unset
        if isinstance(self.action_ref, Unset):
            action_ref = UNSET
        else:
            action_ref = self.action_ref

        conditions: dict[str, Any] | None | Unset
        if isinstance(self.conditions, Unset):
            conditions = UNSET
        elif isinstance(self.conditions, UpdateRuleRequestConditionsType0):
            conditions = self.conditions.to_dict()
        else:
            conditions = self.conditions

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        sensor_worker_affinity: dict[str, Any] | None | Unset
        if isinstance(self.sensor_worker_affinity, Unset):
            sensor_worker_affinity = UNSET
        elif isinstance(
            self.sensor_worker_affinity, UpdateRuleRequestSensorWorkerAffinityType0
        ):
            sensor_worker_affinity = self.sensor_worker_affinity.to_dict()
        else:
            sensor_worker_affinity = self.sensor_worker_affinity

        sensor_worker_selector: dict[str, Any] | None | Unset
        if isinstance(self.sensor_worker_selector, Unset):
            sensor_worker_selector = UNSET
        elif isinstance(
            self.sensor_worker_selector, UpdateRuleRequestSensorWorkerSelectorType0
        ):
            sensor_worker_selector = self.sensor_worker_selector.to_dict()
        else:
            sensor_worker_selector = self.sensor_worker_selector

        sensor_worker_tolerations: list[dict[str, Any]] | None | Unset
        if isinstance(self.sensor_worker_tolerations, Unset):
            sensor_worker_tolerations = UNSET
        elif isinstance(self.sensor_worker_tolerations, list):
            sensor_worker_tolerations = []
            for (
                sensor_worker_tolerations_type_0_item_data
            ) in self.sensor_worker_tolerations:
                sensor_worker_tolerations_type_0_item = (
                    sensor_worker_tolerations_type_0_item_data.to_dict()
                )
                sensor_worker_tolerations.append(sensor_worker_tolerations_type_0_item)

        else:
            sensor_worker_tolerations = self.sensor_worker_tolerations

        trace_tag_template: None | str | Unset
        if isinstance(self.trace_tag_template, Unset):
            trace_tag_template = UNSET
        else:
            trace_tag_template = self.trace_tag_template

        trigger_params: dict[str, Any] | None | Unset
        if isinstance(self.trigger_params, Unset):
            trigger_params = UNSET
        elif isinstance(self.trigger_params, UpdateRuleRequestTriggerParamsType0):
            trigger_params = self.trigger_params.to_dict()
        else:
            trigger_params = self.trigger_params

        trigger_ref: None | str | Unset
        if isinstance(self.trigger_ref, Unset):
            trigger_ref = UNSET
        else:
            trigger_ref = self.trigger_ref

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if action_params is not UNSET:
            field_dict["action_params"] = action_params
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if conditions is not UNSET:
            field_dict["conditions"] = conditions
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if label is not UNSET:
            field_dict["label"] = label
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if sensor_worker_affinity is not UNSET:
            field_dict["sensor_worker_affinity"] = sensor_worker_affinity
        if sensor_worker_selector is not UNSET:
            field_dict["sensor_worker_selector"] = sensor_worker_selector
        if sensor_worker_tolerations is not UNSET:
            field_dict["sensor_worker_tolerations"] = sensor_worker_tolerations
        if trace_tag_template is not UNSET:
            field_dict["trace_tag_template"] = trace_tag_template
        if trigger_params is not UNSET:
            field_dict["trigger_params"] = trigger_params
        if trigger_ref is not UNSET:
            field_dict["trigger_ref"] = trigger_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.update_rule_request_action_params_type_0 import (
            UpdateRuleRequestActionParamsType0,
        )
        from ..models.update_rule_request_conditions_type_0 import (
            UpdateRuleRequestConditionsType0,
        )
        from ..models.update_rule_request_sensor_worker_affinity_type_0 import (
            UpdateRuleRequestSensorWorkerAffinityType0,
        )
        from ..models.update_rule_request_sensor_worker_selector_type_0 import (
            UpdateRuleRequestSensorWorkerSelectorType0,
        )
        from ..models.update_rule_request_sensor_worker_tolerations_type_0_item import (
            UpdateRuleRequestSensorWorkerTolerationsType0Item,
        )
        from ..models.update_rule_request_trigger_params_type_0 import (
            UpdateRuleRequestTriggerParamsType0,
        )

        d = dict(src_dict)

        def _parse_action_params(
            data: object,
        ) -> None | Unset | UpdateRuleRequestActionParamsType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                action_params_type_0 = UpdateRuleRequestActionParamsType0.from_dict(
                    data
                )

                return action_params_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateRuleRequestActionParamsType0, data)

        action_params = _parse_action_params(d.pop("action_params", UNSET))

        def _parse_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        action_ref = _parse_action_ref(d.pop("action_ref", UNSET))

        def _parse_conditions(
            data: object,
        ) -> None | Unset | UpdateRuleRequestConditionsType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                conditions_type_0 = UpdateRuleRequestConditionsType0.from_dict(data)

                return conditions_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateRuleRequestConditionsType0, data)

        conditions = _parse_conditions(d.pop("conditions", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_permission_set_refs(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                permission_set_refs_type_0 = cast(list[str], data)

                return permission_set_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        permission_set_refs = _parse_permission_set_refs(
            d.pop("permission_set_refs", UNSET)
        )

        def _parse_sensor_worker_affinity(
            data: object,
        ) -> None | Unset | UpdateRuleRequestSensorWorkerAffinityType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                sensor_worker_affinity_type_0 = (
                    UpdateRuleRequestSensorWorkerAffinityType0.from_dict(data)
                )

                return sensor_worker_affinity_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateRuleRequestSensorWorkerAffinityType0, data)

        sensor_worker_affinity = _parse_sensor_worker_affinity(
            d.pop("sensor_worker_affinity", UNSET)
        )

        def _parse_sensor_worker_selector(
            data: object,
        ) -> None | Unset | UpdateRuleRequestSensorWorkerSelectorType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                sensor_worker_selector_type_0 = (
                    UpdateRuleRequestSensorWorkerSelectorType0.from_dict(data)
                )

                return sensor_worker_selector_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateRuleRequestSensorWorkerSelectorType0, data)

        sensor_worker_selector = _parse_sensor_worker_selector(
            d.pop("sensor_worker_selector", UNSET)
        )

        def _parse_sensor_worker_tolerations(
            data: object,
        ) -> list[UpdateRuleRequestSensorWorkerTolerationsType0Item] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                sensor_worker_tolerations_type_0 = []
                _sensor_worker_tolerations_type_0 = data
                for (
                    sensor_worker_tolerations_type_0_item_data
                ) in _sensor_worker_tolerations_type_0:
                    sensor_worker_tolerations_type_0_item = (
                        UpdateRuleRequestSensorWorkerTolerationsType0Item.from_dict(
                            sensor_worker_tolerations_type_0_item_data
                        )
                    )

                    sensor_worker_tolerations_type_0.append(
                        sensor_worker_tolerations_type_0_item
                    )

                return sensor_worker_tolerations_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                list[UpdateRuleRequestSensorWorkerTolerationsType0Item] | None | Unset,
                data,
            )

        sensor_worker_tolerations = _parse_sensor_worker_tolerations(
            d.pop("sensor_worker_tolerations", UNSET)
        )

        def _parse_trace_tag_template(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trace_tag_template = _parse_trace_tag_template(
            d.pop("trace_tag_template", UNSET)
        )

        def _parse_trigger_params(
            data: object,
        ) -> None | Unset | UpdateRuleRequestTriggerParamsType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                trigger_params_type_0 = UpdateRuleRequestTriggerParamsType0.from_dict(
                    data
                )

                return trigger_params_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateRuleRequestTriggerParamsType0, data)

        trigger_params = _parse_trigger_params(d.pop("trigger_params", UNSET))

        def _parse_trigger_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        trigger_ref = _parse_trigger_ref(d.pop("trigger_ref", UNSET))

        update_rule_request = cls(
            action_params=action_params,
            action_ref=action_ref,
            conditions=conditions,
            description=description,
            enabled=enabled,
            label=label,
            permission_set_refs=permission_set_refs,
            sensor_worker_affinity=sensor_worker_affinity,
            sensor_worker_selector=sensor_worker_selector,
            sensor_worker_tolerations=sensor_worker_tolerations,
            trace_tag_template=trace_tag_template,
            trigger_params=trigger_params,
            trigger_ref=trigger_ref,
        )

        return update_rule_request
